//! What the app-server says when a turn fails, taken in whole: its typed kind and status before its sentences, the moment a limit ends from the rate limits it reports, and the vendor's words kept as they were said.
//!
//! Protocol fields are flattened here; all decisions use the shared provider classifier.
//! Harness-specific reset times, partial output and retry observations remain intact.

use std::str::FromStr;

use std::time::Duration as Wait;

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use orch_agent_contract::events::{LimitScope, TurnError, TurnLimit, turn_error};
use orch_types::{Json, Time};

/// How many characters of a sentence the adapter's own sentence repeats at most.
const SAID_IN_MESSAGE_CHARS: usize = 300;

/// How long before `now` a date may lie and still be read as a moment ahead: a clock may be off by that much.
const SKEW_ALLOWED: Duration = Duration::hours(36);

/// How long the host waits for a harness before it gives up on a turn: a turn in which the harness says nothing
/// while it waits for the model, and a harness that goes on announcing that it tries again. The app-server has
/// no deadline for the first byte of an answer and, when it cannot reach the vendor, waits for the network
/// with a pause that doubles and no end, so these are the only ends such a turn has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Patience {
    /// How long a turn may go without a word, while nothing but the model is waited for; `None` for ever.
    pub silence: Option<Wait>,
    /// How long the harness may go on trying again, with no answer between; `None` for ever.
    pub retrying: Option<Wait>,
}

/// Fifteen minutes of silence: the longest a model is awaited without a word in any of the turns recorded, with
/// the room a long reasoning takes.
pub const DEFAULT_SILENCE: Wait = Wait::from_secs(15 * 60);

/// Ten minutes of trying again: the app-server's pauses between tries double from six seconds, so it has made
/// the seven tries a person would think reasonable by then.
pub const DEFAULT_RETRYING: Wait = Wait::from_secs(10 * 60);

impl Default for Patience {
    fn default() -> Self {
        Self {
            silence: Some(DEFAULT_SILENCE),
            retrying: Some(DEFAULT_RETRYING),
        }
    }
}

/// What the app-server announced about trying again, and about the answer being written when it failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    /// How many times it said it would try again.
    pub retries: u64,
    /// When it first said so, if it did.
    pub first_retry_at: Option<Time>,
    /// The cause it gave the last time, as it gave it.
    pub last_retry: Option<String>,
    /// Whether the last attempt wrote part of an answer.
    pub streamed: bool,
}

/// The turn's end when the host gives up on a harness that goes on trying again.
#[must_use]
pub fn gave_up_trying(notes: &Notes, stopped_after_secs: i64) -> TurnError {
    let cause =
        orch_provider_contract::failure::classify(&orch_provider_contract::failure::FailureInput {
            message: notes.last_retry.as_deref().unwrap_or_default(),
            ..Default::default()
        });
    let (kind, retryable) = if cause.code == turn_error::HARNESS_ERROR {
        (turn_error::PROVIDER_UNREACHABLE, true)
    } else {
        (cause.code.as_str(), cause.retryable())
    };
    let mut failure = TurnError::new(
        kind,
        "The harness kept retrying the model call and was stopped.",
        retryable,
    )
    .saying(&format!(
        "{}\n(the harness announced {} tries over {stopped_after_secs} seconds and was stopped)",
        notes.last_retry.as_deref().unwrap_or("no cause given"),
        notes.retries
    ))
    .cut_short(notes.streamed);
    failure.reason = cause.reason;
    failure
}

/// The turn's end when the host gives up on a harness that says nothing while it waits for the model.
#[must_use]
pub fn gave_up_waiting(silent_for_secs: i64) -> TurnError {
    TurnError::new(
        turn_error::HARNESS_ERROR,
        "The harness said nothing while it waited for the model, and was stopped.",
        true,
    )
    .saying(&format!(
        "no word from the harness for {silent_for_secs} seconds while it waited for the model"
    ))
}

/// The windows of a plan as the app-server last reported them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Windows {
    /// The shorter window (a few hours).
    pub primary: Option<Window>,
    /// The longer window (a week).
    pub secondary: Option<Window>,
    /// The name of the limit when it is not the plan's own (a family of models).
    pub name: Option<String>,
    /// The harness's id of the limit; the plan's own is `codex`.
    pub id: Option<String>,
}

/// One window of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// How full it is, in percent.
    pub used_percent: i64,
    /// How long it is, in minutes.
    pub minutes: Option<i64>,
    /// When it ends, in seconds since the epoch.
    pub resets_at: Option<i64>,
}

impl Windows {
    /// Reads a `rateLimits` object of `account/rateLimits/updated`.
    #[must_use]
    pub fn read(limits: &Json) -> Self {
        let window = |name: &str| {
            let window = limits.get(name).filter(|window| window.is_object())?;
            Some(Window {
                used_percent: window.get("usedPercent").and_then(Json::as_f64).map_or(0, |percent| {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a percentage of a window is between 0 and 100, far inside an i64"
                    )]
                    let whole = percent.round() as i64;
                    whole
                }),
                minutes: window.get("windowDurationMins").and_then(Json::as_i64),
                resets_at: window.get("resetsAt").and_then(Json::as_i64),
            })
        };
        Self {
            primary: window("primary"),
            secondary: window("secondary"),
            name: limits
                .get("limitName")
                .and_then(Json::as_str)
                .map(str::to_owned),
            id: limits
                .get("limitId")
                .and_then(Json::as_str)
                .map(str::to_owned),
        }
    }

    /// Whether the limit is another than the plan's own: one family of models has its own.
    #[must_use]
    pub fn is_of_part_of_the_plan(&self) -> bool {
        self.name.is_some() || self.id.as_deref().is_some_and(|id| id != PLAN_LIMIT_ID)
    }

    /// The windows that are full.
    fn full(&self) -> impl Iterator<Item = &Window> {
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .filter(|window| window.used_percent >= 100)
    }

    /// When the limit that stopped the request ends: the moment of the full window that ends last, since
    /// nothing runs before every full window has.
    #[must_use]
    pub fn reset_of_full(&self) -> Option<i64> {
        self.full().filter_map(|window| window.resets_at).max()
    }

    /// What the window that stopped the request is called.
    fn full_name(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        match self.full().next().and_then(|window| window.minutes) {
            Some(minutes) if minutes >= 10_000 => "weekly".to_owned(),
            Some(minutes) if minutes >= 1_000 => "daily".to_owned(),
            Some(_) => "session".to_owned(),
            None => "usage".to_owned(),
        }
    }
}

/// The fields of one failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts<'a> {
    /// The name in `codexErrorInfo`, or `other` when it has none.
    pub info: &'a str,
    /// The HTTP status the name carries.
    pub status: Option<i64>,
    /// The message.
    pub message: &'a str,
    /// The cause the app-server adds.
    pub details: Option<&'a str>,
    /// The code of the vendor's JSON body, when the message is one.
    pub body_code: Option<String>,
}

/// The id of the plan's own limit; a different id describes a model family.
const PLAN_LIMIT_ID: &str = "codex";
/// The harness's marker for a stream that broke after it began.
const STREAM_BROKE: &str = "stream disconnected before completion";
/// The harness's marker for a response cut by the output-token limit.
const OUTPUT_LIMIT: &str = "Incomplete response returned";

struct Row {
    name: &'static str,
    code: &'static str,
    retryable: bool,
}

fn shared(facts: &Facts<'_>) -> orch_provider_contract::failure::Classification {
    let message = [Some(facts.message), facts.details]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    orch_provider_contract::failure::classify(&orch_provider_contract::failure::FailureInput {
        codes: [facts.body_code.as_deref(), Some(facts.info)]
            .into_iter()
            .flatten()
            .collect(),
        status: facts.status,
        message: &message,
        transport: facts.status.is_none()
            && matches!(
                facts.info,
                "httpConnectionFailed"
                    | "responseStreamConnectionFailed"
                    | "responseStreamDisconnected"
                    | "responseTooManyFailedAttempts"
            ),
    })
}

fn row_of(facts: &Facts<'_>) -> Row {
    let classified = shared(facts);
    let mut name = classified.row();
    if facts.message.contains(OUTPUT_LIMIT)
        || facts.details.is_some_and(|s| s.contains(OUTPUT_LIMIT))
    {
        name = "output_limit";
    }
    Row {
        name,
        code: if name == "output_limit" {
            turn_error::HARNESS_ERROR
        } else {
            neutral_code(&classified.code)
        },
        retryable: name != "output_limit" && classified.retryable(),
    }
}

fn neutral_code(code: &str) -> &'static str {
    match code {
        "provider_exhausted" => turn_error::PROVIDER_EXHAUSTED,
        "provider_unauthorized" => turn_error::PROVIDER_UNAUTHORIZED,
        "model_unavailable" => turn_error::MODEL_UNAVAILABLE,
        "model_refused" => turn_error::MODEL_REFUSED,
        "context_overflow" => turn_error::CONTEXT_OVERFLOW,
        "provider_busy" => turn_error::PROVIDER_BUSY,
        "provider_unreachable" => turn_error::PROVIDER_UNREACHABLE,
        _ => turn_error::HARNESS_ERROR,
    }
}

/// The fields of an `error` object of the app-server.
#[must_use]
pub fn facts_of(error: &Json) -> Facts<'_> {
    let message = error
        .get("message")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let (info, status) = match error.get("codexErrorInfo") {
        Some(Json::String(name)) => (name.as_str(), None),
        Some(Json::Object(members)) => {
            members
                .iter()
                .next()
                .map_or(("other", None), |(name, detail)| {
                    (
                        name.as_str(),
                        detail.get("httpStatusCode").and_then(Json::as_i64),
                    )
                })
        }
        _ => ("other", None),
    };
    Facts {
        info,
        status,
        message,
        details: error
            .get("additionalDetails")
            .and_then(Json::as_str)
            .filter(|details| !details.is_empty()),
        body_code: body_code(message),
    }
}

/// The `error.code` (or, failing that, `error.type`) of a message that is the vendor's JSON body, which the
/// app-server passes on as it is for a request the vendor refused.
fn body_code(message: &str) -> Option<String> {
    let body: Json = serde_json::from_str(message.trim()).ok()?;
    let error = body.get("error")?;
    ["code", "type"]
        .into_iter()
        .find_map(|member| error.get(member).and_then(Json::as_str))
        .map(str::to_owned)
}

/// The name of the row that finds a failure, for a log and for the tests.
#[must_use]
pub fn row_name(error: &Json) -> &'static str {
    row_of(&facts_of(error)).name
}

/// What a turn that failed is told: the code its row gives, the vendor's words, the moment a limit ends and
/// whether the answer was cut off. `windows` are the plan's windows as the app-server last reported them.
#[must_use]
pub fn classify<Z: TimeZone>(
    error: &Json,
    notes: &Notes,
    windows: Option<&Windows>,
    now: DateTime<Utc>,
    runtime: &Z,
) -> TurnError {
    let facts = facts_of(error);
    let row = row_of(&facts);
    let resets_at = (row.code == turn_error::PROVIDER_EXHAUSTED)
        .then(|| {
            windows
                .and_then(Windows::reset_of_full)
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
                .map(Time::new)
                .or_else(|| reset_in_sentence(facts.message, now, runtime))
        })
        .flatten();
    // A response that began and did not complete is cut short, whether or not any of it was shown.
    let incomplete =
        notes.streamed || row.name == "output_limit" || facts.message.starts_with(STREAM_BROKE);
    // A limit of one family of models is retryable on another; the plan's own is not.
    let retryable = if row.code == turn_error::PROVIDER_EXHAUSTED {
        windows.is_some_and(Windows::is_of_part_of_the_plan)
    } else {
        row.retryable
    };
    let mut failure = TurnError::new(
        row.code,
        &message_of(&row, &facts, windows, resets_at),
        retryable,
    )
    .resetting_at(resets_at)
    .cut_short(incomplete)
    .limited((row.code == turn_error::PROVIDER_EXHAUSTED).then(|| limit_of(windows)));
    if row.code == turn_error::PROVIDER_EXHAUSTED {
        failure.window = windows
            .filter(|windows| !windows.is_of_part_of_the_plan())
            .and_then(|windows| {
                [
                    ("primary", windows.primary.as_ref()),
                    ("secondary", windows.secondary.as_ref()),
                ]
                .into_iter()
                .filter_map(|(id, window)| {
                    window
                        .filter(|window| window.used_percent >= 100)
                        .map(|window| (id, window))
                })
                .max_by_key(|(_, window)| window.resets_at)
                .map(|(id, _)| id.to_owned())
            });
    }
    let said = said_of(&facts, notes, now, row.retryable);
    if !said.trim().is_empty() {
        failure = failure.saying(&said);
    }
    failure.reason = shared(&facts).reason;
    failure.status = facts.status;
    failure
}

/// The limit the plan's windows say stopped the request: one family's where the limit is not the plan's own, else
/// the plan's, over the window that is full.
fn limit_of(windows: Option<&Windows>) -> TurnLimit {
    let Some(windows) = windows else {
        return TurnLimit {
            scope: LimitScope::Plan,
            window_minutes: None,
            model_family: None,
            retry_after_secs: None,
        };
    };
    TurnLimit {
        scope: if windows.is_of_part_of_the_plan() {
            LimitScope::ModelFamily
        } else {
            LimitScope::Plan
        },
        window_minutes: windows.full().next().and_then(|window| window.minutes),
        model_family: windows.name.clone(),
        retry_after_secs: None,
    }
}

/// The words, with the cause the app-server adds, and, where it tried again by itself and gave up, how often.
fn said_of(facts: &Facts<'_>, notes: &Notes, now: DateTime<Utc>, retried_by_it: bool) -> String {
    let mut said = facts.message.to_owned();
    if let Some(details) = facts.details
        && !said.contains(details)
    {
        said.push_str(" (");
        said.push_str(details);
        said.push(')');
    }
    if notes.retries > 0 && retried_by_it {
        let waited = notes
            .first_retry_at
            .map_or(0, |first| (now - first.get()).num_seconds().max(0));
        said.push_str(&format!(
            "\n(the harness tried again {} times over {waited} seconds before it gave up)",
            notes.retries
        ));
    }
    said
}

/// The adapter's own sentence for a failure.
fn message_of(
    row: &Row,
    facts: &Facts<'_>,
    windows: Option<&Windows>,
    resets_at: Option<Time>,
) -> String {
    let until = resets_at.map_or_else(String::new, |time| {
        format!("; it ends {}", time.get().format("%Y-%m-%d %H:%M UTC"))
    });
    match row.name {
        "plan_limit" => format!(
            "The {} limit of the plan is reached{until}.",
            windows.map_or_else(|| "usage".to_owned(), Windows::full_name)
        ),
        "quota" => "The account behind the provider has no balance left.".to_owned(),
        "context_window" => "The conversation no longer fits the model's context.".to_owned(),
        "model_refused" => "The vendor's filter refused this request.".to_owned(),
        "model_access" => "The requested model access has not been granted.".to_owned(),
        "credential_refused" => "The vendor refused the provider's sign-in or key.".to_owned(),
        "model_not_found" => "The vendor does not offer this model to this account.".to_owned(),
        "output_limit" => {
            "The model stopped at its limit for one answer; the answer is cut short.".to_owned()
        }
        "busy" => format!(
            "The vendor's service is busy or failed{}.",
            facts
                .status
                .map_or_else(String::new, |status| format!(" (status {status})"))
        ),
        "out_of_reach" => {
            "The vendor's service could not be reached, or its answer was cut off.".to_owned()
        }
        _ => format!(
            "The harness failed: {}",
            facts
                .message
                .chars()
                .take(SAID_IN_MESSAGE_CHARS)
                .collect::<String>()
        ),
    }
}

// ---------------------------------------------------------------------------------------------------------
// The moment, read from the sentence

/// The moment a limit ends as the harness's sentence says it, for the case that no window does: `try again at
/// 9:02 PM`, `try again at Oct 14th, 2026 6:02 PM`. The harness writes it in its own zone and names none, so
/// it is `runtime`'s. A time without a date is the next one that lies ahead.
#[must_use]
pub fn reset_in_sentence<Z: TimeZone>(text: &str, now: DateTime<Utc>, runtime: &Z) -> Option<Time> {
    let at = text.rfind("try again at ")? + "try again at ".len();
    let tail = text[at..].trim().trim_end_matches('.');
    let tokens: Vec<&str> = tail
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|token| !token.is_empty())
        .collect();
    let [date @ .., clock, meridiem] = tokens.as_slice() else {
        return None;
    };
    let time = clock_time(clock, meridiem)?;
    let local = now.with_timezone(runtime);
    let today = local.date_naive();
    let at = |day: NaiveDate| -> Option<DateTime<Utc>> {
        runtime
            .from_local_datetime(&NaiveDateTime::new(day, time))
            .earliest()
            .map(|moment| moment.with_timezone(&Utc))
    };
    let found = match date {
        [] => {
            let first = at(today)?;
            if first > now {
                first
            } else {
                at(today.succ_opt()?)?
            }
        }
        [month, day, year] => {
            let year = year.parse::<i32>().ok()?;
            let day = day
                .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                .parse::<u32>()
                .ok()?;
            at(NaiveDate::from_ymd_opt(year, month_of(month)?, day)?)?
        }
        [month, day] => {
            let month = month_of(month)?;
            let day = day
                .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                .parse::<u32>()
                .ok()?;
            let this_year = at(NaiveDate::from_ymd_opt(today.year(), month, day)?)?;
            if this_year >= now - SKEW_ALLOWED {
                this_year
            } else {
                at(NaiveDate::from_ymd_opt(today.year() + 1, month, day)?)?
            }
        }
        _ => return None,
    };
    Some(Time::new(found))
}

/// `9:02` and `PM`.
fn clock_time(clock: &str, meridiem: &str) -> Option<NaiveTime> {
    let (hour, minute) = clock.split_once(':')?;
    let hour = hour.parse::<u32>().ok()?;
    let minute = minute.parse::<u32>().ok()?;
    if !(1..=12).contains(&hour) {
        return None;
    }
    let evening = match meridiem.to_ascii_uppercase().as_str() {
        "PM" => true,
        "AM" => false,
        _ => return None,
    };
    NaiveTime::from_hms_opt(hour % 12 + if evening { 12 } else { 0 }, minute, 0)
}

fn month_of(token: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = token.to_ascii_lowercase();
    let short = lower.get(..3)?;
    MONTHS
        .iter()
        .position(|month| *month == short)
        .and_then(|at| u32::try_from(at + 1).ok())
}

/// A zone by its IANA name, for the tests of other modules that hold a moment in one.
#[must_use]
pub fn zone_named(name: &str) -> Option<chrono_tz::Tz> {
    chrono_tz::Tz::from_str(name).ok()
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;
    use serde_json::json;

    use super::*;

    fn utc(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    fn singapore() -> FixedOffset {
        FixedOffset::east_opt(8 * 3600).unwrap()
    }

    fn error(info: Json, message: &str) -> Json {
        json!({ "message": message, "codexErrorInfo": info, "additionalDetails": null })
    }

    #[test]
    fn a_limit_is_read_from_the_window_that_is_full_and_not_from_the_sentence() {
        let windows = Windows::read(&json!({
            "limitId": "codex", "limitName": null,
            "primary": { "usedPercent": 100, "windowDurationMins": 300, "resetsAt": 1_791_810_152 },
            "secondary": { "usedPercent": 40, "windowDurationMins": 10080, "resetsAt": null }
        }));
        let failure = error(
            json!("usageLimitExceeded"),
            "You\u{2019}ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or try again at 9:02 PM.",
        );
        let got = classify(
            &failure,
            &Notes::default(),
            Some(&windows),
            utc("2026-10-12T10:00:00Z"),
            &singapore(),
        );
        assert_eq!(got.code, "provider_exhausted");
        assert!(!got.retryable);
        assert_eq!(
            got.resets_at.map(|t| t.get()),
            Some(utc("2026-10-12T13:02:32Z"))
        );
        assert!(got.message.contains("session limit"), "{}", got.message);
        assert!(got.said.unwrap().contains("try again at 9:02 PM"));
    }

    #[test]
    fn a_limit_of_one_family_of_models_can_be_tried_on_another_model_and_the_plans_own_cannot() {
        let now = utc("2026-10-12T10:00:00Z");
        let failure = error(
            json!("usageLimitExceeded"),
            "You\u{2019}ve hit your usage limit. Try again later.",
        );
        let window =
            json!({ "usedPercent": 100, "windowDurationMins": 300, "resetsAt": 1_791_810_152 });
        let own =
            Windows::read(&json!({ "limitId": "codex", "limitName": null, "primary": window }));
        let family = Windows::read(
            &json!({ "limitId": "codex_spark", "limitName": "Spark", "primary": window }),
        );
        let retryable = |windows: &Windows| {
            classify(&failure, &Notes::default(), Some(windows), now, &Utc).retryable
        };
        assert!(!retryable(&own));
        assert!(retryable(&family));
        assert!(
            !classify(&failure, &Notes::default(), None, now, &Utc).retryable,
            "no word of a family is the plan's own"
        );
    }

    #[test]
    fn when_both_windows_are_full_the_limit_ends_with_the_later_one() {
        let windows = Windows::read(&json!({
            "primary": { "usedPercent": 100, "windowDurationMins": 300, "resetsAt": 1_791_810_152 },
            "secondary": { "usedPercent": 100, "windowDurationMins": 10080, "resetsAt": 1_791_972_152 }
        }));
        assert_eq!(windows.reset_of_full(), Some(1_791_972_152));
        assert_eq!(
            windows.full_name(),
            "session",
            "the window that is first in line names the limit"
        );
    }

    #[test]
    fn the_sentence_is_read_in_the_runtimes_zone_when_no_window_gives_a_moment() {
        let now = utc("2026-10-12T10:00:00Z");
        let at = |text: &str| reset_in_sentence(text, now, &singapore()).map(|time| time.get());
        assert_eq!(
            at("or try again at Oct 14th, 2026 6:02 PM."),
            Some(utc("2026-10-14T10:02:00Z"))
        );
        assert_eq!(
            at("or try again at 9:02 PM."),
            Some(utc("2026-10-12T13:02:00Z"))
        );
        assert_eq!(
            at("or try again at 3:45 PM."),
            Some(utc("2026-10-13T07:45:00Z"))
        );
        assert_eq!(
            at("or try again at Nov 1st, 2026 6:02 PM."),
            Some(utc("2026-11-01T10:02:00Z"))
        );
        assert_eq!(at("or try again later."), None);
        assert_eq!(at("or try again at 25:00 PM."), None);
    }

    #[test]
    fn a_failure_with_the_harnesss_name_for_the_vendors_trouble_is_busy_or_out_of_reach_by_its_status()
     {
        let now = utc("2026-10-12T10:00:00Z");
        let code = |info: Json, message: &str| {
            classify(&error(info, message), &Notes::default(), None, now, &Utc).code
        };
        assert_eq!(
            code(json!("serverOverloaded"), "Selected model is at capacity."),
            "provider_busy"
        );
        assert_eq!(
            code(json!("internalServerError"), "high demand"),
            "provider_busy"
        );
        assert_eq!(
            code(json!("rateLimitExceeded"), "rate limit exceeded"),
            "provider_busy"
        );
        assert_eq!(
            code(
                json!({ "responseTooManyFailedAttempts": { "httpStatusCode": 429 } }),
                "exceeded retry limit, last status: 429 Too Many Requests"
            ),
            "provider_busy"
        );
        assert_eq!(
            code(
                json!({ "httpConnectionFailed": { "httpStatusCode": 502 } }),
                "unexpected status 502 Bad Gateway"
            ),
            "provider_busy"
        );
        assert_eq!(
            code(
                json!({ "httpConnectionFailed": { "httpStatusCode": 401 } }),
                "unexpected status 401 Unauthorized"
            ),
            "provider_unauthorized"
        );
        assert_eq!(
            code(
                json!({ "httpConnectionFailed": { "httpStatusCode": 404 } }),
                "unexpected status 404 Not Found"
            ),
            "harness_error"
        );
        assert_eq!(
            code(
                json!({ "httpConnectionFailed": { "httpStatusCode": 413 } }),
                "unexpected status 413 Payload Too Large"
            ),
            "context_overflow"
        );
        assert_eq!(
            code(
                json!({ "responseStreamDisconnected": { "httpStatusCode": null } }),
                "Reconnecting... waiting for network"
            ),
            "provider_unreachable"
        );
        assert_eq!(
            code(
                json!("other"),
                "stream disconnected before completion: stream closed before response.completed"
            ),
            "provider_unreachable"
        );
    }

    #[test]
    fn a_bad_request_is_read_by_the_code_of_the_vendors_body_that_the_harness_passes_on() {
        let now = utc("2026-10-12T10:00:00Z");
        let body = |code: &str| {
            json!({ "error": { "message": "m", "type": "invalid_request_error", "param": null, "code": code } }).to_string()
        };
        let code = |code: &str| {
            classify(
                &error(json!("other"), &body(code)),
                &Notes::default(),
                None,
                now,
                &Utc,
            )
            .code
        };
        assert_eq!(code("context_length_exceeded"), "context_overflow");
        assert_eq!(code("model_not_found"), "model_unavailable");
        assert_eq!(code("invalid_api_key"), "provider_unauthorized");
        assert_eq!(code("invalid_value"), "harness_error");
    }

    #[test]
    fn a_quota_that_ran_out_is_a_refusal_of_the_account_and_not_a_limit_that_ends() {
        let now = utc("2026-10-12T10:00:00Z");
        let failure = error(
            json!("usageLimitExceeded"),
            "Quota exceeded. Check your plan and billing details.",
        );
        let got = classify(&failure, &Notes::default(), None, now, &Utc);
        assert_eq!(
            (got.code.as_str(), got.resets_at),
            ("provider_exhausted", None)
        );
    }

    #[test]
    fn what_no_row_knows_keeps_the_words_and_changes_nothing() {
        let now = utc("2026-10-12T10:00:00Z");
        let failure = json!({ "message": "A refusal of next year", "codexErrorInfo": { "somethingNew": { "n": 1 } },
                              "additionalDetails": "because" });
        let got = classify(&failure, &Notes::default(), None, now, &Utc);
        assert_eq!(got.code, "harness_error");
        assert!(!got.retryable && !got.incomplete);
        assert_eq!(
            got.said.as_deref(),
            Some("A refusal of next year (because)")
        );
        assert_eq!(
            classify(&json!({}), &Notes::default(), None, now, &Utc).said,
            None
        );
    }

    #[test]
    fn a_stream_that_broke_after_part_of_an_answer_is_incomplete_and_says_how_often_it_was_tried_again()
     {
        let now = utc("2026-10-12T10:00:30Z");
        let notes = Notes {
            retries: 5,
            first_retry_at: Some(Time::new(utc("2026-10-12T10:00:00Z"))),
            last_retry: None,
            streamed: true,
        };
        let failure = error(
            json!("other"),
            "stream disconnected before completion: Transport error: network error",
        );
        let got = classify(&failure, &notes, None, now, &Utc);
        assert_eq!(
            (got.code.as_str(), got.incomplete, got.retryable),
            ("provider_unreachable", true, true)
        );
        assert!(
            got.said
                .unwrap()
                .contains("tried again 5 times over 30 seconds")
        );
        let limit = error(
            json!("other"),
            "stream disconnected before completion: Incomplete response returned, reason: max_output_tokens",
        );
        let got = classify(&limit, &Notes::default(), None, now, &Utc);
        assert_eq!(
            (got.code.as_str(), got.incomplete, got.retryable),
            ("harness_error", true, false)
        );
    }
}

#[cfg(test)]
mod stopped_retries {
    use super::*;

    #[test]
    fn the_native_retry_deadline_retains_the_cause_for_the_recovery_rule() {
        for (message, kind, reason) in [
            ("Model at capacity, please retry", "provider_busy", None),
            (
                "flagged for possible cybersecurity risk",
                "model_refused",
                Some("cyber"),
            ),
            (
                "Daybreak isn't available for this model",
                "model_unavailable",
                Some("access"),
            ),
            ("no native cause given", "provider_unreachable", None),
        ] {
            let failure = gave_up_trying(
                &Notes {
                    last_retry: Some(message.to_owned()),
                    streamed: true,
                    ..Default::default()
                },
                600,
            );
            assert_eq!(failure.code, kind, "{message}");
            assert_eq!(failure.reason.as_deref(), reason, "{message}");
            assert!(failure.incomplete);
        }
    }
}
