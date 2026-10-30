//! What the harness says when a turn fails, taken in whole: its typed fields before its sentences, the moment a limit ends, and the vendor's words kept as they were said.
//!
//! Protocol fields are flattened here; all decisions use the shared provider classifier.
//! Harness-specific reset times, partial output and retry observations remain intact.

use std::str::FromStr;

use chrono::{
    DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use orch_agent_contract::events::{LimitScope, TurnError, TurnLimit, turn_error};
use orch_types::{Json, Time};

use crate::stream::Frame;

/// A moment given as a number is in seconds; above this many it is in milliseconds. The largest moment in
/// seconds anyone will write (year 5138) is smaller than the smallest in milliseconds that matters (year 1973).
const MILLISECONDS_ABOVE: i64 = 100_000_000_000;

/// How long before `now` a date without a year may lie and still be this year's: a reset is in the future,
/// so a date further back than a clock may be off by is next year's.
const SKEW_ALLOWED: Duration = Duration::hours(36);

/// How many characters of a sentence the adapter's own sentence repeats at most.
const SAID_IN_MESSAGE_CHARS: usize = 300;

/// What the stream said about a failure while the turn ran, kept until its `result` arrives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    /// A message or stream delta whose documented stop reason is a vendor refusal.
    pub refusal: Option<Json>,
    /// The last `rate_limit_info`: of a `rate_limit_event`, or carried by the typed error.
    pub limit: Option<LimitInfo>,
    /// What the harness's last API-error message of the turn said.
    pub message: Option<ErrorMessage>,
    /// What it announced about trying again.
    pub retries: Retries,
    /// Whether the answer that was being written when it failed was cut off.
    pub cut: bool,
}

/// A `rate_limit_info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitInfo {
    /// `allowed`, `allowed_warning` or `rejected`.
    pub status: Option<String>,
    /// When the window ends, in seconds since the epoch.
    pub resets_at: Option<i64>,
    /// Which window: `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`, `overage`.
    pub kind: Option<String>,
}

impl LimitInfo {
    /// Reads a `rate_limit_info` object.
    #[must_use]
    pub fn read(info: &Json) -> Self {
        let text = |member: &str| info.get(member).and_then(Json::as_str).map(str::to_owned);
        Self {
            status: text("status"),
            resets_at: info.get("resetsAt").and_then(Json::as_i64),
            kind: text("rateLimitType"),
        }
    }

    /// Whether the window that stopped the request is named: a rejection that says which or until when.
    fn names_a_window(&self) -> bool {
        self.status.as_deref() == Some("rejected")
            && (self.resets_at.is_some() || self.kind.is_some())
    }
}

/// The assistant message that wraps an API error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMessage {
    /// The harness's class of the error (`error`).
    pub class: Option<String>,
    /// The typed kind (`api_error`), when it has one.
    pub typed: Option<String>,
    /// The HTTP status (`api_error_status`), when an answer came.
    pub status: Option<i64>,
    /// Its text.
    pub text: String,
}

/// What the harness announced about trying again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Retries {
    /// How many times it tried again.
    pub attempts: u64,
    /// How long it had decided to wait between its tries, all together, in milliseconds.
    pub waited_ms: u64,
    /// The status the last try failed with, when an answer came.
    pub last_status: Option<i64>,
    /// The harness's class of the last failure.
    pub last_kind: Option<String>,
}

/// The turn's end when native retries run out of patience, retaining the last known cause.
#[must_use]
pub fn gave_up_trying(retries: &Retries, stopped_after_secs: u64) -> TurnError {
    let last = retries.last();
    let cause =
        orch_provider_contract::failure::classify(&orch_provider_contract::failure::FailureInput {
            codes: retries.last_kind.as_deref().into_iter().collect(),
            status: retries.last_status,
            message: &last,
            transport: retries.last_status.is_none(),
        });
    let (kind, retryable) = if cause.code == turn_error::HARNESS_ERROR {
        (turn_error::PROVIDER_UNREACHABLE, true)
    } else {
        (cause.code.as_str(), cause.retryable())
    };
    let mut failure=TurnError::new(kind,"The harness kept retrying the model call and was stopped.",retryable)
    .saying(&format!(
        "the harness announced {} tries, the last ended with {last}, over {stopped_after_secs} seconds, and was stopped",
        retries.attempts,
    ));
    failure.reason = cause.reason;
    failure.status = retries.last_status;
    failure
}

/// The turn's end when the host gives up on a harness that says nothing while it waits for the model.
#[must_use]
pub fn gave_up_waiting(silent_for_secs: u64) -> TurnError {
    TurnError::new(
        turn_error::HARNESS_ERROR,
        "The harness said nothing while it waited for the model, and was stopped.",
        true,
    )
    .saying(&format!(
        "no word from the harness for {silent_for_secs} seconds while it waited for the model"
    ))
}

impl Retries {
    /// One more `api_retry` message.
    pub fn note(&mut self, frame: &Frame) {
        let attempt = frame.get("attempt").and_then(Json::as_u64).unwrap_or(0);
        self.attempts = self.attempts.max(attempt);
        self.waited_ms += frame
            .get("retry_delay_ms")
            .and_then(Json::as_u64)
            .unwrap_or(0);
        self.last_status = frame.get("error_status").and_then(Json::as_i64);
        self.last_kind = frame.text("error").map(str::to_owned);
    }

    /// What the last of the tries failed with, in the harness's own terms.
    #[must_use]
    pub fn last(&self) -> String {
        match (self.last_status, &self.last_kind) {
            (Some(status), Some(kind)) => format!("status {status} ({kind})"),
            (Some(status), None) => format!("status {status}"),
            (None, Some(kind)) => format!("no answer ({kind})"),
            (None, None) => "no answer".to_owned(),
        }
    }
}

/// The fields of one failure, flattened: the `result`'s where it has them, the assistant message's otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts<'a> {
    /// The HTTP status; `None` when no answer came.
    pub status: Option<i64>,
    /// The harness's class of the error.
    pub class: Option<&'a str>,
    /// The typed kind.
    pub typed: Option<&'a str>,
    /// Why the turn ended (`terminal_reason`).
    pub reason: Option<&'a str>,
    /// The window and moment of a limit.
    pub limit: Option<&'a LimitInfo>,
    /// The sentence.
    pub text: &'a str,
}

struct Row {
    name: &'static str,
    code: &'static str,
    retryable: bool,
}

fn shared(facts: &Facts<'_>) -> orch_provider_contract::failure::Classification {
    let mut codes: Vec<_> = [
        facts.typed,
        facts.class,
        facts.reason.filter(|reason| *reason != "api_error"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if facts.limit.is_some_and(LimitInfo::names_a_window) && facts.status == Some(429) {
        codes.insert(0, "usage_limit_reached");
    }
    orch_provider_contract::failure::classify(&orch_provider_contract::failure::FailureInput {
        codes,
        status: facts.status,
        message: facts.text,
        // api_error is also the wrapper for local stops, including the output-token cap.
        // Only an actual transport type or the retained words establishes a network failure.
        transport: false,
    })
}

fn row_of(facts: &Facts<'_>) -> Row {
    let classified = shared(facts);
    let name = match classified.row() {
        "context_window" => "prompt_too_long",
        "plan_limit" if facts.class == Some("billing_error") || facts.status == Some(402) => {
            "balance_ran_out"
        }
        "plan_limit"
            if facts.typed.is_none() && !facts.limit.is_some_and(LimitInfo::names_a_window) =>
        {
            "plan_limit_said"
        }
        "unknown"
            if facts.class == Some("max_output_tokens")
                || facts.typed == Some("max_output_tokens") =>
        {
            "output_limit"
        }
        name => name,
    };
    Row {
        name,
        code: neutral_code(&classified.code),
        retryable: classified.retryable(),
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

/// The fields of the failure, from the `result` and what the stream said before it.
fn facts_of<'a>(frame: &'a Frame, notes: &'a Notes, sentence: &'a str) -> Facts<'a> {
    let message = notes.message.as_ref();
    Facts {
        status: frame
            .get("api_error_status")
            .and_then(Json::as_i64)
            .or_else(|| message.and_then(|message| message.status)),
        class: message.and_then(|message| message.class.as_deref()),
        typed: frame
            .text("api_error")
            .or_else(|| message.and_then(|message| message.typed.as_deref())),
        reason: frame.text("terminal_reason"),
        limit: notes.limit.as_ref(),
        text: sentence,
    }
}

/// The words of the failure as the harness said them: the sentence of the `result`, else that of the error
/// message, else what its `errors` list holds.
#[must_use]
pub fn sentence_of(frame: &Frame, notes: &Notes) -> String {
    let from_errors = || {
        frame
            .get("errors")
            .and_then(Json::as_array)
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(Json::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default()
    };
    [
        frame.text("result").unwrap_or_default().to_owned(),
        notes
            .message
            .as_ref()
            .map(|message| message.text.clone())
            .unwrap_or_default(),
        from_errors(),
    ]
    .into_iter()
    .find(|text| !text.trim().is_empty())
    .unwrap_or_default()
}

/// What a turn that failed is told: the code its row gives, the vendor's words, the moment a limit ends and
/// whether the answer was cut off. `now` and `runtime` are for the one sentence that is read for a time.
#[must_use]
pub fn classify<Z: TimeZone>(
    frame: &Frame,
    notes: &Notes,
    now: DateTime<Utc>,
    runtime: &Z,
) -> TurnError {
    if let Some(body) = notes.refusal.as_ref()
        && let Some((classification, words)) =
            orch_provider_contract::failure::completed_refusal(body)
    {
        let mut error = TurnError::new(
            turn_error::MODEL_REFUSED,
            "The vendor's filter refused this request.",
            true,
        )
        .saying(&words)
        .cut_short(notes.cut);
        error.reason = classification.reason;
        return error;
    }
    let sentence = sentence_of(frame, notes);
    let facts = facts_of(frame, notes, &sentence);
    let row = row_of(&facts);
    let resets_at = (row.code == turn_error::PROVIDER_EXHAUSTED)
        .then(|| {
            facts
                .limit
                .and_then(|limit| limit.resets_at)
                .and_then(moment)
                .or_else(|| reset_in_sentence(&sentence, now, runtime))
        })
        .flatten();
    let incomplete = notes.cut
        || row.name == "output_limit"
        || sentence.to_ascii_lowercase().contains("may be incomplete");
    let retryable = if row.code == turn_error::PROVIDER_EXHAUSTED {
        limit_covers_part_of_the_plan(&facts)
    } else {
        row.retryable
    };
    let mut error = TurnError::new(
        row.code,
        &message_of(&row, &facts, resets_at, &sentence),
        retryable,
    )
    .resetting_at(resets_at)
    .cut_short(incomplete)
    .limited((row.code == turn_error::PROVIDER_EXHAUSTED).then(|| {
        limit_of(
            facts.limit.and_then(|limit| limit.kind.as_deref()),
            retryable,
        )
    }));
    if row.code == turn_error::PROVIDER_EXHAUSTED {
        error.window = facts
            .limit
            .and_then(|limit| match limit.kind.as_deref() {
                Some("five_hour") => Some("primary"),
                Some("seven_day") => Some("secondary"),
                Some("seven_day_opus") => Some("secondary:opus"),
                Some("seven_day_sonnet") => Some("secondary:sonnet"),
                _ => None,
            })
            .map(str::to_owned);
    }
    if !sentence.trim().is_empty() {
        error = error.saying(&said(&sentence, &notes.retries, row.retryable));
    }
    error.reason = shared(&facts).reason;
    error.status = facts.status;
    error
}

/// The limit a `rate_limit_info` names by its window: the plan's session or week, one family's week, or the
/// spending. Without the field, a limit that leaves part of the plan (`part`) is a family's, else the plan's.
fn limit_of(kind: Option<&str>, part: bool) -> TurnLimit {
    const WEEK: i64 = 7 * 24 * 60;
    let (scope, window_minutes, model_family) = match kind {
        Some("five_hour") => (LimitScope::Plan, Some(5 * 60), None),
        Some("seven_day" | "seven_day_overage_included") => (LimitScope::Plan, Some(WEEK), None),
        Some("seven_day_opus") => (LimitScope::ModelFamily, Some(WEEK), Some("opus")),
        Some("seven_day_sonnet") => (LimitScope::ModelFamily, Some(WEEK), Some("sonnet")),
        Some("overage") => (LimitScope::Spending, None, None),
        _ if part => (LimitScope::ModelFamily, None, None),
        _ => (LimitScope::Plan, None, None),
    };
    TurnLimit {
        scope,
        window_minutes,
        model_family: model_family.map(str::to_owned),
        retry_after_secs: None,
    }
}

/// Whether the limit that was reached covers one family of models or the spending and not the whole plan.
fn limit_covers_part_of_the_plan(facts: &Facts<'_>) -> bool {
    match facts.limit.and_then(|limit| limit.kind.as_deref()) {
        Some("seven_day_opus" | "seven_day_sonnet" | "overage") => true,
        Some(_) => false,
        // No field: the word the sentence puts between "your" and "limit".
        None => {
            let plain = facts.text.replace('\u{2019}', "'");
            if plain.starts_with("You're out of usage credits") {
                return true;
            }
            // A sentence that names no word ("You've hit your limit") is the plan's own limit.
            plain
                .strip_prefix("You've hit your ")
                .and_then(|rest| rest.split_once(" limit"))
                .is_some_and(|(word, _)| !matches!(word, "session" | "weekly"))
        }
    }
}

/// The words, and where the harness tried again by itself and gave up, how often.
fn said(sentence: &str, retries: &Retries, retried_by_it: bool) -> String {
    if retries.attempts == 0 || !retried_by_it {
        return sentence.to_owned();
    }
    format!(
        "{sentence}\n(the harness tried again {} times, waiting {} seconds in all, before it gave up)",
        retries.attempts,
        retries.waited_ms / 1000
    )
}

/// A moment given as a number of seconds, or of milliseconds.
fn moment(number: i64) -> Option<Time> {
    let seconds = if number > MILLISECONDS_ABOVE {
        number / 1000
    } else {
        number
    };
    DateTime::from_timestamp(seconds, 0).map(Time::new)
}

/// The name of the row that finds a failure, for a log and for the tests.
#[must_use]
pub fn row_name(frame: &Frame, notes: &Notes) -> &'static str {
    if notes.refusal.is_some() {
        return "model_refused";
    }
    let sentence = sentence_of(frame, notes);
    row_of(&facts_of(frame, notes, &sentence)).name
}

/// The adapter's own sentence for a failure: what happened, in English, for whoever reads the thread.
fn message_of(row: &Row, facts: &Facts<'_>, resets_at: Option<Time>, sentence: &str) -> String {
    let until = resets_at.map_or_else(String::new, |time| {
        format!("; it ends {}", time.get().format("%Y-%m-%d %H:%M UTC"))
    });
    match row.name {
        "plan_limit" | "plan_limit_said" => format!(
            "The {} limit of the plan is reached{until}.",
            window_name(
                facts.limit.and_then(|limit| limit.kind.as_deref()),
                sentence
            )
        ),
        "prompt_too_long" => "The conversation no longer fits the model's context.".to_owned(),
        "balance_ran_out" => "The account behind the provider has no balance left.".to_owned(),
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
            sentence
                .chars()
                .take(SAID_IN_MESSAGE_CHARS)
                .collect::<String>()
        ),
    }
}

/// What the window is called in the adapter's sentence.
fn window_name(kind: Option<&str>, sentence: &str) -> String {
    match kind {
        Some("five_hour") => "session".to_owned(),
        Some("seven_day" | "seven_day_overage_included") => "weekly".to_owned(),
        Some("seven_day_opus") => "weekly Opus".to_owned(),
        Some("seven_day_sonnet") => "weekly Sonnet".to_owned(),
        Some("overage") => "spending".to_owned(),
        Some(other) => other.replace('_', " "),
        // Only the sentence is left: the word between "your" and "limit".
        None => sentence
            .replace('\u{2019}', "'")
            .strip_prefix("You've hit your ")
            .and_then(|rest| rest.split_once(" limit"))
            .map_or_else(|| "usage".to_owned(), |(word, _)| word.to_owned()),
    }
}

// ---------------------------------------------------------------------------------------------------------
// The moment, read from the sentence

/// The moment a limit ends as the sentence says it, for the case that the harness gave no field: `resets
/// 3:45pm`, `resets Mon 12:00am`, `resets Oct 14, 5pm (Asia/Singapore)`. The zone in brackets is the one
/// the time is in; without one it is `runtime`'s. A date without a year is the next one that lies ahead.
#[must_use]
pub fn reset_in_sentence<Z: TimeZone>(text: &str, now: DateTime<Utc>, runtime: &Z) -> Option<Time> {
    let at = text.rfind("resets ")? + "resets ".len();
    let mut tail = text[at..].trim().trim_end_matches('.');
    let mut named: Option<Tz> = None;
    if let (Some(open), Some(close)) = (tail.find('('), tail.rfind(')'))
        && open < close
    {
        named = Tz::from_str(tail[open + 1..close].trim()).ok();
        tail = tail[..open].trim();
    }
    let tokens: Vec<&str> = tail
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|token| !token.is_empty())
        .collect();
    let (time_token, date_tokens) = tokens.split_last()?;
    let time = clock_time(time_token)?;
    match named {
        Some(zone) => wall_clock(&zone, date_tokens, time, now),
        None => wall_clock(runtime, date_tokens, time, now),
    }
}

/// `5pm`, `5:59pm`, `12:00am`.
fn clock_time(token: &str) -> Option<NaiveTime> {
    let lower = token.to_ascii_lowercase();
    let (digits, evening) = if let Some(digits) = lower.strip_suffix("pm") {
        (digits, true)
    } else {
        (lower.strip_suffix("am")?, false)
    };
    let (hour, minute) = match digits.split_once(':') {
        Some((hour, minute)) => (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?),
        None => (digits.parse::<u32>().ok()?, 0),
    };
    if !(1..=12).contains(&hour) {
        return None;
    }
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

fn weekday_of(token: &str) -> Option<Weekday> {
    Weekday::from_str(token.get(..3)?).ok()
}

/// The day, in `zone`, that the date part of the sentence means, and the moment at `time` on it.
fn wall_clock<Z: TimeZone>(
    zone: &Z,
    date: &[&str],
    time: NaiveTime,
    now: DateTime<Utc>,
) -> Option<Time> {
    let local_now = now.with_timezone(zone);
    let today = local_now.date_naive();
    let at = |day: NaiveDate| -> Option<DateTime<Utc>> {
        zone.from_local_datetime(&NaiveDateTime::new(day, time))
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
        [weekday] => {
            let wanted = weekday_of(weekday)?;
            (0..=7)
                .filter_map(|ahead| today.checked_add_signed(Duration::days(ahead)))
                .filter(|day| day.weekday() == wanted)
                .find_map(|day| at(day).filter(|moment| *moment > now))?
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

    fn result(extra: Json) -> Frame {
        let mut base = json!({ "type": "result", "subtype": "success", "is_error": true, "terminal_reason": "api_error" });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        Frame::from_value(base)
    }

    #[test]
    fn the_three_forms_of_the_vendors_sentence_are_read_in_the_zone_it_names_or_the_runtime() {
        // Monday 12 October 2026, 18:00 in Singapore.
        let now = utc("2026-10-12T10:00:00Z");
        let at = |text: &str| reset_in_sentence(text, now, &singapore()).map(|time| time.get());
        // The given sentence of the eight stopped agents: a date, a time without minutes, and a zone.
        assert_eq!(
            at("You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)"),
            Some(utc("2026-10-14T09:00:00Z"))
        );
        // A time alone is today's, or tomorrow's when today's has passed; here in the runtime's zone.
        assert_eq!(
            at("You've hit your session limit · resets 8:59pm"),
            Some(utc("2026-10-12T12:59:00Z"))
        );
        assert_eq!(
            at("You've hit your session limit · resets 3:45pm"),
            Some(utc("2026-10-13T07:45:00Z"))
        );
        // A weekday and a time: the next such day.
        assert_eq!(
            at("You've hit your weekly limit · resets Mon 12:00am"),
            Some(utc("2026-10-18T16:00:00Z"))
        );
        assert_eq!(
            at("You've hit your weekly limit · resets Fri 9:30am"),
            Some(utc("2026-10-16T01:30:00Z"))
        );
        // A zone the sentence names wins over the runtime's.
        assert_eq!(
            at("You've hit your weekly limit · resets Oct 14, 9:59am (UTC)"),
            Some(utc("2026-10-14T09:59:00Z"))
        );
        assert_eq!(
            at("You've hit your weekly limit · resets Oct 14, 5:59am (America/New_York)"),
            Some(utc("2026-10-14T09:59:00Z"))
        );
    }

    #[test]
    fn a_date_without_a_year_is_the_next_one_that_lies_ahead() {
        let now = utc("2026-12-30T10:00:00Z");
        assert_eq!(
            reset_in_sentence("resets Jan 2, 5pm (UTC)", now, &Utc).map(|time| time.get()),
            Some(utc("2027-01-02T17:00:00Z"))
        );
        assert_eq!(
            reset_in_sentence("resets Dec 31, 5pm (UTC)", now, &Utc).map(|time| time.get()),
            Some(utc("2026-12-31T17:00:00Z"))
        );
    }

    #[test]
    fn a_sentence_that_gives_no_moment_or_one_that_cannot_be_read_gives_none() {
        let now = utc("2026-10-12T10:00:00Z");
        for text in [
            "You've hit your session limit",
            "resets soon",
            "resets 25:00pm",
            "resets Oct 32, 5pm",
            "resets Blursday 5pm",
            "",
        ] {
            assert_eq!(reset_in_sentence(text, now, &Utc), None, "{text:?}");
        }
        // A zone nobody knows is the runtime's.
        assert_eq!(
            reset_in_sentence("resets 11:30pm (Mars/Olympus)", now, &Utc).map(|time| time.get()),
            Some(utc("2026-10-12T23:30:00Z"))
        );
    }

    #[test]
    fn a_moment_in_milliseconds_is_read_as_one() {
        assert_eq!(
            moment(1_791_968_400).map(|t| t.get()),
            Some(utc("2026-10-14T09:00:00Z"))
        );
        assert_eq!(
            moment(1_791_968_400_000).map(|t| t.get()),
            Some(utc("2026-10-14T09:00:00Z"))
        );
    }

    #[test]
    fn a_limit_of_the_plan_is_found_by_the_harnesss_own_kind_with_the_moment_from_its_event() {
        let notes = Notes {
            limit: Some(LimitInfo {
                status: Some("rejected".into()),
                resets_at: Some(1_791_968_400),
                kind: Some("seven_day".into()),
            }),
            ..Notes::default()
        };
        let frame = result(json!({
            "api_error_status": 429, "api_error": "usage_limit_reached",
            "result": "You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)"
        }));
        let error = classify(&frame, &notes, utc("2026-10-12T10:00:00Z"), &Utc);
        assert_eq!(error.code, "provider_exhausted");
        assert!(!error.retryable);
        assert_eq!(
            error.resets_at.map(|time| time.get()),
            Some(utc("2026-10-14T09:00:00Z"))
        );
        assert_eq!(
            error.said.as_deref(),
            Some("You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)")
        );
        assert!(error.message.contains("weekly"), "{}", error.message);
        assert!(
            error.message.contains("2026-10-14 09:00 UTC"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_limit_of_one_family_or_of_spending_can_be_tried_on_another_model_and_the_plans_own_cannot()
    {
        let now = utc("2026-10-12T10:00:00Z");
        let with = |kind: &str| {
            let notes = Notes {
                limit: Some(LimitInfo {
                    status: Some("rejected".into()),
                    resets_at: Some(1_791_968_400),
                    kind: Some(kind.into()),
                }),
                ..Notes::default()
            };
            let frame = result(
                json!({ "api_error_status": 429, "api_error": "usage_limit_reached", "result": "You've hit your limit" }),
            );
            classify(&frame, &notes, now, &Utc).retryable
        };
        for whole in ["five_hour", "seven_day", "seven_day_overage_included"] {
            assert!(!with(whole), "{whole}");
        }
        for part in ["seven_day_opus", "seven_day_sonnet", "overage"] {
            assert!(with(part), "{part}");
        }
        // With no field, the word between "your" and "limit" says it.
        let said = |text: &str| {
            classify(
                &result(json!({ "api_error_status": 429, "result": text })),
                &Notes::default(),
                now,
                &Utc,
            )
            .retryable
        };
        assert!(!said("You've hit your session limit"));
        assert!(!said(
            "You\u{2019}ve hit your weekly limit \u{b7} resets 3pm"
        ));
        assert!(said("You've hit your Opus limit \u{b7} resets 3pm"));
        assert!(said("You're out of usage credits \u{b7} resets 3pm"));
    }

    #[test]
    fn a_harness_that_leaves_the_fields_out_is_still_read_by_the_start_of_its_sentence() {
        let frame = result(json!({
            "api_error_status": 429, "result": "You've hit your session limit · resets 3:45pm (UTC)"
        }));
        let error = classify(&frame, &Notes::default(), utc("2026-10-12T10:00:00Z"), &Utc);
        assert_eq!(error.code, "provider_exhausted");
        assert_eq!(
            error.resets_at.map(|time| time.get()),
            Some(utc("2026-10-12T15:45:00Z"))
        );
        // The same words on any other status are no limit.
        let frame =
            result(json!({ "api_error_status": 500, "result": "You've hit your session limit" }));
        assert_eq!(
            classify(&frame, &Notes::default(), utc("2026-10-12T10:00:00Z"), &Utc).code,
            "provider_busy"
        );
    }

    #[test]
    fn a_throttling_that_is_not_the_plans_is_busy_even_when_the_event_says_rejected() {
        let notes = Notes {
            refusal: None,
            limit: Some(LimitInfo {
                status: Some("rejected".into()),
                resets_at: None,
                kind: None,
            }),
            message: Some(ErrorMessage {
                class: Some("rate_limit".into()),
                typed: None,
                status: Some(429),
                text: "API Error: Server is temporarily limiting requests (not your usage limit)"
                    .into(),
            }),
            retries: Retries {
                attempts: 10,
                waited_ms: 147_000,
                last_status: Some(429),
                last_kind: Some("rate_limit".into()),
            },
            cut: false,
        };
        let frame = result(json!({
            "api_error_status": 429,
            "result": "API Error: Server is temporarily limiting requests (not your usage limit) · too many"
        }));
        let error = classify(&frame, &notes, utc("2026-10-12T10:00:00Z"), &Utc);
        assert_eq!(error.code, "provider_busy");
        assert!(error.retryable);
        assert_eq!(error.resets_at, None);
        let said = error.said.unwrap();
        assert!(
            said.starts_with("API Error: Server is temporarily limiting requests"),
            "{said}"
        );
        assert!(said.contains("tried again 10 times"), "{said}");
    }

    #[test]
    fn a_failure_no_row_knows_is_a_harness_error_that_keeps_the_words_and_changes_nothing() {
        let frame = result(
            json!({ "api_error_status": 418, "api_error": "a_kind_of_next_year", "result": "I am a teapot" }),
        );
        let error = classify(&frame, &Notes::default(), utc("2026-10-12T10:00:00Z"), &Utc);
        assert_eq!(error.code, "harness_error");
        assert!(!error.retryable);
        assert_eq!(error.said.as_deref(), Some("I am a teapot"));
        assert_eq!(error.resets_at, None);
        assert!(!error.incomplete);
        // A result that says nothing at all of why it is an error.
        let frame = Frame::from_value(
            json!({ "type": "result", "subtype": "error_during_execution", "is_error": true }),
        );
        let error = classify(&frame, &Notes::default(), utc("2026-10-12T10:00:00Z"), &Utc);
        assert_eq!((error.code.as_str(), error.said), ("harness_error", None));
    }

    #[test]
    fn a_cut_answer_and_a_stop_at_the_output_limit_are_marked_incomplete() {
        let cut = Notes {
            cut: true,
            ..Notes::default()
        };
        let frame =
            result(json!({ "result": "API Error: Connection to the API was lost (ECONNRESET)" }));
        let error = classify(&frame, &cut, utc("2026-10-12T10:00:00Z"), &Utc);
        assert_eq!(
            (error.code.as_str(), error.incomplete),
            ("provider_unreachable", true)
        );
        let frame =
            result(json!({ "api_error": "max_output_tokens", "result": "The answer was cut" }));
        let error = classify(&frame, &Notes::default(), utc("2026-10-12T10:00:00Z"), &Utc);
        assert_eq!(
            (error.code.as_str(), error.incomplete),
            ("harness_error", true)
        );
        let frame = result(
            json!({ "api_error_status": 500, "result": "The response above may be incomplete." }),
        );
        assert!(classify(&frame, &Notes::default(), utc("2026-10-12T10:00:00Z"), &Utc).incomplete);
    }
}

#[cfg(test)]
mod stopped_retries {
    use super::*;

    #[test]
    fn native_retry_exhaustion_retains_structured_cause_and_status() {
        for (native, status, kind, reason) in [
            ("overloaded_error", 503, "provider_busy", None),
            ("server_error", 504, "provider_unreachable", None),
            ("content_filter", 403, "model_refused", Some("safety")),
            (
                "model_access_denied",
                403,
                "model_unavailable",
                Some("access"),
            ),
        ] {
            let failure = gave_up_trying(
                &Retries {
                    last_kind: Some(native.to_owned()),
                    last_status: Some(status),
                    ..Default::default()
                },
                600,
            );
            assert_eq!(failure.code, kind);
            assert_eq!(failure.reason.as_deref(), reason);
            assert_eq!(failure.status, Some(status));
        }
    }
}
