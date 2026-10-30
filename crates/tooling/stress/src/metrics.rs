//! What the drive measured: every request by route, every page, the live feeds' lag, the turns, the sockets.
//!
//! Requests made while seeding and signing in are kept apart from those of the measured window, so that the bars are
//! judged on the steady drive alone.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A read's p95 latency target, in milliseconds.
pub const READ_BAR_MS: f64 = 300.0;
/// A write's p95 latency target, in milliseconds.
pub const WRITE_BAR_MS: f64 = 1000.0;
/// The live feeds' p95 lag target, in milliseconds.
pub const LAG_BAR_MS: f64 = 2000.0;
/// Maximum statement duration, in milliseconds.
pub const STATEMENT_BAR_MS: f64 = 1000.0;

/// The latencies and failures of one route (`METHOD /path/{template}`).
#[derive(Default)]
struct Route {
    ms: Vec<f64>,
    errors: BTreeMap<String, u64>,
}

#[derive(Default)]
struct Turns {
    sent: u64,
    blocked: BTreeMap<String, u64>,
    refreshed: u64,
    refused: BTreeMap<String, u64>,
    ended: BTreeMap<String, u64>,
    completed_ms: Vec<f64>,
    timed_out: u64,
}

#[derive(Default)]
struct Inner {
    setup: BTreeMap<String, Route>,
    drive: BTreeMap<String, Route>,
    pages: BTreeMap<String, Vec<f64>>,
    lag: BTreeMap<String, Vec<f64>>,
    turns: Turns,
    sockets: BTreeMap<String, u64>,
    ping_ms: Vec<f64>,
    notes: Vec<String>,
}

/// Collects measurements from every task of the drive.
#[derive(Default)]
pub struct Recorder {
    measuring: AtomicBool,
    inner: Mutex<Inner>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Recorder {
    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut inner)
    }

    /// Whether the measured window is open.
    pub fn measuring(&self) -> bool {
        self.measuring.load(Ordering::Relaxed)
    }

    /// Opens or closes the measured window.
    pub fn set_measuring(&self, on: bool) {
        self.measuring.store(on, Ordering::Relaxed);
    }

    /// One HTTP request: its route, its status (0 when no answer came), the API's error code, how long it took.
    pub fn request(&self, route: &str, status: u16, code: Option<&str>, took_ms: f64) {
        let measuring = self.measuring();
        self.with(|inner| {
            let table = if measuring {
                &mut inner.drive
            } else {
                &mut inner.setup
            };
            let entry = table.entry(route.to_owned()).or_default();
            entry.ms.push(took_ms);
            if !(200..300).contains(&status) {
                let key = match code {
                    Some(code) => format!("{status} {code}"),
                    None => status.to_string(),
                };
                *entry.errors.entry(key).or_default() += 1;
            }
        });
    }

    /// One page opened: all its requests made at once, timed until the last answered.
    pub fn page(&self, kind: &str, took: Duration) {
        if self.measuring() {
            self.with(|inner| {
                inner
                    .pages
                    .entry(kind.to_owned())
                    .or_default()
                    .push(ms(took))
            });
        }
    }

    /// How long after it was sent a change reached a socket that follows its feed.
    pub fn lag(&self, kind: &str, took: Duration) {
        if self.measuring() {
            self.with(|inner| inner.lag.entry(kind.to_owned()).or_default().push(ms(took)));
        }
    }

    /// An input was sent to a thread.
    pub fn turn_sent(&self) {
        if self.measuring() {
            self.with(|inner| inner.turns.sent += 1);
        }
    }

    /// An input was refused (status and code).
    pub fn turn_refused(&self, why: String) {
        if self.measuring() {
            self.with(|inner| *inner.turns.refused.entry(why).or_default() += 1);
        }
    }

    /// The turn of an input ended: its state (and failure code), and how long since the input was sent.
    pub fn turn_ended(&self, how: String, took: Duration, completed: bool) {
        if self.measuring() {
            self.with(|inner| {
                *inner.turns.ended.entry(how).or_default() += 1;
                if completed {
                    inner.turns.completed_ms.push(ms(took));
                }
            });
        }
    }

    /// The thread of a waiting input said it is blocked, and why.
    pub fn turn_blocked(&self, why: String) {
        if self.measuring() {
            self.with(|inner| *inner.turns.blocked.entry(why).or_default() += 1);
        }
    }

    /// The drive asked a blocked thread's provider to be looked at again.
    pub fn refreshed(&self) {
        if self.measuring() {
            self.with(|inner| inner.turns.refreshed += 1);
        }
    }

    /// No end of the turn was seen in time.
    pub fn turn_timed_out(&self) {
        if self.measuring() {
            self.with(|inner| inner.turns.timed_out += 1);
        }
    }

    /// A socket event: connected, a frame of an op, a resync, an error code, a disconnection.
    pub fn socket(&self, what: &str) {
        self.with(|inner| *inner.sockets.entry(what.to_owned()).or_default() += 1);
    }

    /// The round trip of a socket's ping.
    pub fn ping(&self, took: Duration) {
        if self.measuring() {
            self.with(|inner| inner.ping_ms.push(ms(took)));
        }
    }

    /// Something the report says in words (what could not be made, what is missing).
    pub fn note(&self, text: String) {
        eprintln!("note: {text}");
        self.with(|inner| inner.notes.push(text));
    }

    /// The report of what was measured.
    pub fn report(&self) -> Measured {
        self.with(|inner| Measured {
            routes: rows(&inner.drive),
            setup_routes: rows(&inner.setup),
            pages: inner
                .pages
                .iter()
                .map(|(kind, v)| Spread::of(kind, v, None))
                .collect(),
            lag: inner
                .lag
                .iter()
                .map(|(kind, v)| Spread::of(kind, v, Some(LAG_BAR_MS)))
                .collect(),
            turns: TurnReport {
                sent: inner.turns.sent,
                blocked: inner.turns.blocked.clone(),
                providers_refreshed: inner.turns.refreshed,
                refused: inner.turns.refused.clone(),
                ended: inner.turns.ended.clone(),
                timed_out: inner.turns.timed_out,
                completed: Spread::of(
                    "input sent to turn completed",
                    &inner.turns.completed_ms,
                    None,
                ),
            },
            sockets: inner.sockets.clone(),
            ping: Spread::of("socket ping round trip", &inner.ping_ms, None),
            notes: inner.notes.clone(),
        })
    }
}

fn rows(table: &BTreeMap<String, Route>) -> Vec<RouteRow> {
    let mut out: Vec<RouteRow> = table
        .iter()
        .map(|(route, r)| {
            let write = !route.starts_with("GET ");
            let bar = if write { WRITE_BAR_MS } else { READ_BAR_MS };
            let spread = Spread::of(route, &r.ms, Some(bar));
            RouteRow {
                route: route.clone(),
                class: if write { "write" } else { "read" },
                n: spread.n,
                p50: spread.p50,
                p95: spread.p95,
                p99: spread.p99,
                max: spread.max,
                errors: r.errors.clone(),
                bar_ms: bar,
                within_bar: spread.p95 < bar,
            }
        })
        .collect();
    out.sort_by(|a, b| b.p95.total_cmp(&a.p95));
    out
}

/// The nearest-rank percentile of sorted values.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// How a set of durations is spread.
#[derive(Debug, Clone, Serialize)]
pub struct Spread {
    pub name: String,
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    /// How many were over the bar, where there is one.
    pub over_bar: usize,
}

impl Spread {
    fn of(name: &str, values: &[f64], bar: Option<f64>) -> Self {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        Self {
            name: name.to_owned(),
            n: sorted.len(),
            p50: percentile(&sorted, 50.0),
            p95: percentile(&sorted, 95.0),
            p99: percentile(&sorted, 99.0),
            max: sorted.last().copied().unwrap_or(0.0),
            over_bar: bar.map_or(0, |bar| sorted.iter().filter(|v| **v >= bar).count()),
        }
    }
}

/// One route's measurements and whether its p95 is within its bar.
#[derive(Debug, Clone, Serialize)]
pub struct RouteRow {
    pub route: String,
    pub class: &'static str,
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub errors: BTreeMap<String, u64>,
    pub bar_ms: f64,
    pub within_bar: bool,
}

/// What became of the inputs sent to threads.
#[derive(Debug, Clone, Serialize)]
pub struct TurnReport {
    pub sent: u64,
    /// How often a thread said it is blocked while an input waited, by why.
    pub blocked: BTreeMap<String, u64>,
    /// How often the drive refreshed a blocked thread's provider (`--refresh-blocked`).
    pub providers_refreshed: u64,
    pub refused: BTreeMap<String, u64>,
    pub ended: BTreeMap<String, u64>,
    pub timed_out: u64,
    pub completed: Spread,
}

/// Everything measured.
#[derive(Debug, Clone, Serialize)]
pub struct Measured {
    pub routes: Vec<RouteRow>,
    pub setup_routes: Vec<RouteRow>,
    pub pages: Vec<Spread>,
    pub lag: Vec<Spread>,
    pub turns: TurnReport,
    pub sockets: BTreeMap<String, u64>,
    pub ping: Spread,
    pub notes: Vec<String>,
}
