//! The errors of the two interfaces as the scripted server gives them: status, document, headers and retry hint for each kind, and the error event inside a stream.
//!
//! The Responses format reports an error as `{"error": {"message", "type", "param", "code"}}`, a rate limit
//! as 429 with `rate_limit_error`, an overloaded model as 503 with `service_unavailable_error` and
//! `server_is_overloaded`, both with a `Retry-After` of whole seconds. The Messages format reports it as
//! `{"type": "error", "error": {"type", "message"}, "request_id"}`, with the types the vendor documents for
//! each status, and 529 for an overload. Inside a stream both send an `error` event.
//!
//! **A limit of a plan** is the one error the vendors say more about in headers than in the document, and
//! the harnesses write their sentence ("You've hit your weekly limit · resets …") from the headers. The
//! Messages format names the window and the moment in the `anthropic-ratelimit-unified-*` headers, the
//! Responses format of the ChatGPT plans in `x-codex-*` headers and in the members `plan_type`, `resets_at`
//! and `resets_in_seconds` of an error of the type `usage_limit_reached`. The names are the ones the two
//! harnesses read; a plan limit has no `Retry-After`, because waiting a few seconds does not end it.

use http::StatusCode;
use serde_json::{Value, json};

use super::request::Format;
use super::script::{
    DEFAULT_RETRY_AFTER_SECS, ErrorKind, ErrorSpec, LimitSpec, LimitWindow, Reset,
};

/// The status the Messages format answers with while it is overloaded; the registry of HTTP statuses has no
/// name for it.
const OVERLOADED_STATUS: u16 = 529;

/// What the Responses format says of one kind: status, type, code, message.
struct Responses {
    status: u16,
    kind: &'static str,
    code: &'static str,
    message: &'static str,
}

/// What the Messages format says of one kind: status, type, message.
struct Messages {
    status: u16,
    kind: &'static str,
    message: &'static str,
}

fn responses(kind: ErrorKind) -> Responses {
    let (status, error_type, code, message) = match kind {
        ErrorKind::RateLimit => (
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "rate_limit_error",
            "rate_limit_exceeded",
            "Rate limit reached for requests. Please try again later.",
        ),
        ErrorKind::Overloaded => (
            StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            "service_unavailable_error",
            "server_is_overloaded",
            "The model is temporarily overloaded. Please retry your request after a short delay.",
        ),
        ErrorKind::InvalidRequest => (
            StatusCode::BAD_REQUEST.as_u16(),
            "invalid_request_error",
            "invalid_value",
            "Invalid request.",
        ),
        ErrorKind::Authentication => (
            StatusCode::UNAUTHORIZED.as_u16(),
            "invalid_request_error",
            "invalid_api_key",
            "Incorrect API key provided.",
        ),
        ErrorKind::Permission => (
            StatusCode::FORBIDDEN.as_u16(),
            "invalid_request_error",
            "permission_denied",
            "You are not allowed to do this.",
        ),
        ErrorKind::NotFound => (
            StatusCode::NOT_FOUND.as_u16(),
            "invalid_request_error",
            "model_not_found",
            "The model does not exist or you do not have access to it.",
        ),
        ErrorKind::TooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
            "invalid_request_error",
            "request_too_large",
            "Request body too large.",
        ),
        ErrorKind::Server => (
            StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            "server_error",
            "server_error",
            "The server had an error while processing your request.",
        ),
        ErrorKind::Timeout => (
            StatusCode::GATEWAY_TIMEOUT.as_u16(),
            "server_error",
            "timeout",
            "The request timed out.",
        ),
        ErrorKind::Billing => (
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "insufficient_quota",
            "insufficient_quota",
            "You exceeded your current quota, please check your plan and billing details.",
        ),
        ErrorKind::ContextLength => (
            StatusCode::BAD_REQUEST.as_u16(),
            "invalid_request_error",
            "context_length_exceeded",
            "Your input exceeds the context window of this model. Please adjust your input and try again.",
        ),
        ErrorKind::PlanLimit => (
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "usage_limit_reached",
            "usage_limit_reached",
            "The usage limit has been reached",
        ),
    };
    Responses {
        status,
        kind: error_type,
        code,
        message,
    }
}

fn messages(kind: ErrorKind) -> Messages {
    let (status, error_type, message) = match kind {
        ErrorKind::RateLimit => (
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "rate_limit_error",
            "Number of request tokens has exceeded your per-minute rate limit.",
        ),
        ErrorKind::Overloaded => (OVERLOADED_STATUS, "overloaded_error", "Overloaded"),
        ErrorKind::InvalidRequest => (
            StatusCode::BAD_REQUEST.as_u16(),
            "invalid_request_error",
            "Invalid request.",
        ),
        ErrorKind::Authentication => (
            StatusCode::UNAUTHORIZED.as_u16(),
            "authentication_error",
            "invalid x-api-key",
        ),
        ErrorKind::Permission => (
            StatusCode::FORBIDDEN.as_u16(),
            "permission_error",
            "Your API key does not have permission to use the specified resource.",
        ),
        ErrorKind::NotFound => (
            StatusCode::NOT_FOUND.as_u16(),
            "not_found_error",
            "The requested resource could not be found.",
        ),
        ErrorKind::TooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
            "request_too_large",
            "Request exceeds the maximum allowed number of bytes.",
        ),
        ErrorKind::Server => (
            StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            "api_error",
            "Internal server error",
        ),
        ErrorKind::Timeout => (
            StatusCode::GATEWAY_TIMEOUT.as_u16(),
            "timeout_error",
            "Request timed out.",
        ),
        ErrorKind::Billing => (
            StatusCode::PAYMENT_REQUIRED.as_u16(),
            "billing_error",
            "Your credit balance is too low to access the API.",
        ),
        ErrorKind::ContextLength => (
            StatusCode::BAD_REQUEST.as_u16(),
            "invalid_request_error",
            "prompt is too long: 213462 tokens > 200000 maximum",
        ),
        ErrorKind::PlanLimit => (
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "rate_limit_error",
            "This request would exceed your account's rate limit. Please try again later.",
        ),
    };
    Messages {
        status,
        kind: error_type,
        message,
    }
}

/// An error as a whole answer: its status, its document, its headers and its retry hint.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorReply {
    /// The HTTP status.
    pub status: u16,
    /// The document.
    pub body: Value,
    /// The seconds of `Retry-After`, if the answer carries one.
    pub retry_after: Option<u64>,
    /// The other headers the answer carries: the windows of a plan, for a limit of one.
    pub headers: Vec<(String, String)>,
}

/// The minutes of the shorter window of a plan: five hours.
const SESSION_WINDOW_MINUTES: u32 = 300;

/// The minutes of the longer window of a plan: a week.
const WEEKLY_WINDOW_MINUTES: u32 = 10_080;

/// How full a window is that is not the one that stopped the request.
const OTHER_WINDOW_PERCENT: u32 = 40;

/// The moment a limit ends, in seconds since the epoch, once the clock is known.
fn reset_epoch(limit: &LimitSpec, now: i64) -> Option<i64> {
    limit.resets.map(|reset| match reset {
        Reset::At(at) => at,
        Reset::In(after) => now + after,
    })
}

/// The headers the Messages format names a limit of a plan with: the status, the window that stopped the
/// request (its claim), the moment, and how full the two windows are.
fn unified_headers(limit: &LimitSpec, now: i64) -> Vec<(String, String)> {
    let reset = reset_epoch(limit, now);
    let claim = match limit.window {
        LimitWindow::Session => "five_hour",
        LimitWindow::Weekly => "seven_day",
        LimitWindow::WeeklyOpus => "seven_day_opus",
        LimitWindow::WeeklySonnet => "seven_day_sonnet",
        LimitWindow::Spend => "overage",
    };
    let mut headers = vec![
        (
            "anthropic-ratelimit-unified-status".to_owned(),
            "rejected".to_owned(),
        ),
        (
            "anthropic-ratelimit-unified-representative-claim".to_owned(),
            claim.to_owned(),
        ),
    ];
    let session = limit.window == LimitWindow::Session;
    let full = |is_full: bool| if is_full { "1.0" } else { "0.4" }.to_owned();
    headers.push((
        "anthropic-ratelimit-unified-5h-utilization".to_owned(),
        full(session),
    ));
    headers.push((
        "anthropic-ratelimit-unified-7d-utilization".to_owned(),
        full(!session),
    ));
    if let Some(reset) = reset {
        headers.push((
            "anthropic-ratelimit-unified-reset".to_owned(),
            reset.to_string(),
        ));
        let window = if session { "5h" } else { "7d" };
        headers.push((
            format!("anthropic-ratelimit-unified-{window}-reset"),
            reset.to_string(),
        ));
    }
    if limit.window == LimitWindow::Spend {
        headers.push((
            "anthropic-ratelimit-unified-overage-status".to_owned(),
            "rejected".to_owned(),
        ));
        headers.push((
            "anthropic-ratelimit-unified-overage-disabled-reason".to_owned(),
            "out_of_credits".to_owned(),
        ));
    }
    headers
}

/// The headers a ChatGPT plan names its windows with: how full each is, how long, and when it ends. The
/// window that stopped the request is full.
fn codex_headers(limit: &LimitSpec, now: i64) -> Vec<(String, String)> {
    let reset = reset_epoch(limit, now);
    let session = matches!(limit.window, LimitWindow::Session | LimitWindow::Spend);
    let percent = |is_full: bool| if is_full { 100 } else { OTHER_WINDOW_PERCENT }.to_string();
    let mut headers = vec![
        ("x-codex-active-limit".to_owned(), "codex".to_owned()),
        ("x-codex-primary-used-percent".to_owned(), percent(session)),
        (
            "x-codex-primary-window-minutes".to_owned(),
            SESSION_WINDOW_MINUTES.to_string(),
        ),
        (
            "x-codex-secondary-used-percent".to_owned(),
            percent(!session),
        ),
        (
            "x-codex-secondary-window-minutes".to_owned(),
            WEEKLY_WINDOW_MINUTES.to_string(),
        ),
    ];
    if let Some(reset) = reset {
        let window = if session { "primary" } else { "secondary" };
        headers.push((format!("x-codex-{window}-reset-at"), reset.to_string()));
    }
    headers
}

/// The seconds of `Retry-After` an error carries: a rate limit and an overload always do, with a default,
/// unless the script leaves it out; any other kind only when the script gives one.
fn retry_after(wanted: &ErrorSpec) -> Option<u64> {
    match wanted.retry_after {
        Some(given) => given,
        None => matches!(wanted.kind, ErrorKind::RateLimit | ErrorKind::Overloaded)
            .then_some(DEFAULT_RETRY_AFTER_SECS),
    }
}

/// The answer to a request that fails with `wanted`; `seq` numbers the request and `now` is the moment it
/// is answered, in seconds since the epoch, from which a limit that ends after so many seconds is dated.
#[must_use]
pub fn reply(format: Format, wanted: &ErrorSpec, seq: u64, now: i64) -> ErrorReply {
    let retry_after = retry_after(wanted);
    let plan_limit = wanted.kind == ErrorKind::PlanLimit;
    match format {
        Format::Responses | Format::Chat => {
            let known = responses(wanted.kind);
            let mut error = json!({
                "message": wanted.message.as_deref().unwrap_or(known.message),
                "type": known.kind, "param": null, "code": known.code
            });
            if plan_limit {
                let reset = reset_epoch(&wanted.limit, now);
                error["code"] = Value::Null;
                error["plan_type"] = Value::from(wanted.limit.plan.clone());
                error["resets_at"] = reset.map_or(Value::Null, Value::from);
                error["resets_in_seconds"] =
                    reset.map_or(Value::Null, |reset| Value::from((reset - now).max(0)));
            }
            ErrorReply {
                status: wanted.status.unwrap_or(known.status),
                body: json!({ "error": error }),
                retry_after,
                headers: if plan_limit {
                    codex_headers(&wanted.limit, now)
                } else {
                    Vec::new()
                },
            }
        }
        Format::Messages => {
            let known = messages(wanted.kind);
            ErrorReply {
                status: wanted.status.unwrap_or(known.status),
                body: json!({
                    "type": "error",
                    "error": {
                        "type": known.kind,
                        "message": wanted.message.as_deref().unwrap_or(known.message)
                    },
                    "request_id": format!("req_mock_{seq:06}")
                }),
                retry_after,
                headers: if plan_limit {
                    unified_headers(&wanted.limit, now)
                } else {
                    Vec::new()
                },
            }
        }
    }
}

/// The error event inside a stream: the event's name and its data. `sequence_number` is the next of the
/// Responses format's stream and unused by the other.
#[must_use]
pub fn stream_event(
    format: Format,
    wanted: &ErrorSpec,
    sequence_number: u64,
) -> (&'static str, Value) {
    match format {
        Format::Chat => {
            let known = responses(wanted.kind);
            (
                "error",
                json!({ "error": {
                    "message": wanted.message.as_deref().unwrap_or(known.message),
                    "type": known.kind, "param": null, "code": known.code
                } }),
            )
        }
        Format::Responses => {
            let known = responses(wanted.kind);
            (
                "error",
                json!({
                    "type": "error", "code": known.code,
                    "message": wanted.message.as_deref().unwrap_or(known.message),
                    "param": null, "sequence_number": sequence_number
                }),
            )
        }
        Format::Messages => {
            let known = messages(wanted.kind);
            (
                "error",
                json!({ "type": "error", "error": {
                    "type": known.kind,
                    "message": wanted.message.as_deref().unwrap_or(known.message)
                } }),
            )
        }
    }
}

/// The code a failed response of the Responses format names.
#[must_use]
pub fn response_code(wanted: &ErrorSpec) -> &'static str {
    responses(wanted.kind).code
}

/// The message of an error.
#[must_use]
pub fn message(format: Format, wanted: &ErrorSpec) -> String {
    wanted.message.clone().unwrap_or_else(|| match format {
        Format::Responses | Format::Chat => responses(wanted.kind).message.to_owned(),
        Format::Messages => messages(wanted.kind).message.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The moment every test below answers at: the day this was written.
    const NOW: i64 = 1_791_676_800;

    fn wanted(kind: ErrorKind) -> ErrorSpec {
        ErrorSpec::of(kind)
    }

    fn header<'a>(reply: &'a ErrorReply, name: &str) -> Option<&'a str> {
        reply
            .headers
            .iter()
            .find(|(given, _)| given == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn a_rate_limit_is_429_with_the_vendors_type_in_each_format_and_a_retry_hint() {
        let responses = reply(Format::Responses, &wanted(ErrorKind::RateLimit), 1, NOW);
        assert_eq!(responses.status, 429);
        assert_eq!(responses.body["error"]["type"], "rate_limit_error");
        assert_eq!(responses.body["error"]["code"], "rate_limit_exceeded");
        assert_eq!(responses.retry_after, Some(DEFAULT_RETRY_AFTER_SECS));
        let messages = reply(Format::Messages, &wanted(ErrorKind::RateLimit), 1, NOW);
        assert_eq!(messages.status, 429);
        assert_eq!(messages.body["type"], "error");
        assert_eq!(messages.body["error"]["type"], "rate_limit_error");
        assert_eq!(messages.body["request_id"], "req_mock_000001");
        assert_eq!(messages.retry_after, Some(DEFAULT_RETRY_AFTER_SECS));
    }

    #[test]
    fn overloaded_is_503_in_the_responses_format_and_529_in_the_messages_format() {
        let responses = reply(Format::Responses, &wanted(ErrorKind::Overloaded), 1, NOW);
        assert_eq!(responses.status, 503);
        assert_eq!(responses.body["error"]["type"], "service_unavailable_error");
        assert_eq!(responses.body["error"]["code"], "server_is_overloaded");
        let messages = reply(Format::Messages, &wanted(ErrorKind::Overloaded), 1, NOW);
        assert_eq!(messages.status, 529);
        assert_eq!(messages.body["error"]["type"], "overloaded_error");
        assert_eq!(messages.body["error"]["message"], "Overloaded");
    }

    #[test]
    fn an_invalid_request_and_a_bad_credential_are_400_and_401() {
        assert_eq!(
            reply(
                Format::Responses,
                &wanted(ErrorKind::InvalidRequest),
                1,
                NOW
            )
            .status,
            400
        );
        let auth = reply(
            Format::Responses,
            &wanted(ErrorKind::Authentication),
            1,
            NOW,
        );
        assert_eq!(
            (auth.status, auth.body["error"]["code"].as_str()),
            (401, Some("invalid_api_key"))
        );
        let auth = reply(Format::Messages, &wanted(ErrorKind::Authentication), 1, NOW);
        assert_eq!(auth.status, 401);
        assert_eq!(auth.body["error"]["type"], "authentication_error");
        assert_eq!(auth.retry_after, None);
    }

    #[test]
    fn every_kind_has_a_status_and_a_document_in_both_formats() {
        for kind in ErrorKind::ALL {
            for format in [Format::Responses, Format::Messages] {
                let reply = reply(format, &wanted(kind), 7, NOW);
                assert!((400..600).contains(&reply.status), "{kind:?} {format:?}");
                assert!(reply.body.is_object());
            }
        }
    }

    #[test]
    fn the_script_can_change_the_message_the_status_and_the_retry_hint() {
        let given = ErrorSpec {
            message: Some("boom".into()),
            retry_after: Some(Some(9)),
            status: Some(502),
            ..ErrorSpec::of(ErrorKind::Server)
        };
        let reply = reply(Format::Responses, &given, 1, NOW);
        assert_eq!(reply.status, 502);
        assert_eq!(reply.body["error"]["message"], "boom");
        assert_eq!(reply.retry_after, Some(9));
        let silent = ErrorSpec {
            retry_after: Some(None),
            ..wanted(ErrorKind::RateLimit)
        };
        assert_eq!(
            super::reply(Format::Messages, &silent, 1, NOW).retry_after,
            None
        );
    }

    #[test]
    fn a_prompt_that_is_too_long_is_a_400_that_says_so_in_the_words_of_each_vendor() {
        let responses = reply(Format::Responses, &wanted(ErrorKind::ContextLength), 1, NOW);
        assert_eq!(responses.status, 400);
        assert_eq!(responses.body["error"]["code"], "context_length_exceeded");
        let messages = reply(Format::Messages, &wanted(ErrorKind::ContextLength), 1, NOW);
        assert_eq!(messages.status, 400);
        assert_eq!(messages.body["error"]["type"], "invalid_request_error");
        assert!(
            messages.body["error"]["message"]
                .as_str()
                .is_some_and(|text| text.starts_with("prompt is too long")),
            "{messages:?}"
        );
        assert_eq!((responses.retry_after, messages.retry_after), (None, None));
    }

    #[test]
    fn a_limit_of_a_plan_in_the_messages_format_names_its_window_and_its_moment_in_headers() {
        let mut limit = wanted(ErrorKind::PlanLimit);
        limit.limit.window = LimitWindow::Weekly;
        limit.limit.resets = Some(Reset::At(NOW + 172_800));
        let reply = reply(Format::Messages, &limit, 3, NOW);
        assert_eq!(reply.status, 429);
        assert_eq!(reply.body["error"]["type"], "rate_limit_error");
        assert_eq!(
            reply.retry_after, None,
            "waiting a few seconds does not end a plan's limit"
        );
        assert_eq!(
            header(&reply, "anthropic-ratelimit-unified-status"),
            Some("rejected")
        );
        assert_eq!(
            header(&reply, "anthropic-ratelimit-unified-representative-claim"),
            Some("seven_day")
        );
        let reset = (NOW + 172_800).to_string();
        assert_eq!(
            header(&reply, "anthropic-ratelimit-unified-reset"),
            Some(reset.as_str())
        );
        assert_eq!(
            header(&reply, "anthropic-ratelimit-unified-7d-reset"),
            Some(reset.as_str())
        );
        assert_eq!(
            header(&reply, "anthropic-ratelimit-unified-7d-utilization"),
            Some("1.0")
        );
        assert_eq!(
            header(&reply, "anthropic-ratelimit-unified-5h-utilization"),
            Some("0.4")
        );
        for (window, claim) in [
            (LimitWindow::Session, "five_hour"),
            (LimitWindow::WeeklyOpus, "seven_day_opus"),
            (LimitWindow::WeeklySonnet, "seven_day_sonnet"),
            (LimitWindow::Spend, "overage"),
        ] {
            limit.limit.window = window;
            let reply = super::reply(Format::Messages, &limit, 3, NOW);
            assert_eq!(
                header(&reply, "anthropic-ratelimit-unified-representative-claim"),
                Some(claim)
            );
        }
        limit.limit.resets = None;
        let silent = super::reply(Format::Messages, &limit, 3, NOW);
        assert_eq!(
            header(&silent, "anthropic-ratelimit-unified-reset"),
            None,
            "a vendor may give no time"
        );
    }

    #[test]
    fn a_limit_of_a_plan_in_the_responses_format_is_a_usage_limit_with_the_plan_and_the_moment() {
        let mut limit = wanted(ErrorKind::PlanLimit);
        limit.limit.plan = "plus".into();
        limit.limit.resets = Some(Reset::In(7200));
        let reply = reply(Format::Responses, &limit, 3, NOW);
        assert_eq!(reply.status, 429);
        assert_eq!(reply.body["error"]["type"], "usage_limit_reached");
        assert_eq!(reply.body["error"]["plan_type"], "plus");
        assert_eq!(reply.body["error"]["resets_at"], NOW + 7200);
        assert_eq!(reply.body["error"]["resets_in_seconds"], 7200);
        assert_eq!(reply.retry_after, None);
        assert_eq!(header(&reply, "x-codex-primary-used-percent"), Some("100"));
        assert_eq!(header(&reply, "x-codex-secondary-used-percent"), Some("40"));
        let reset = (NOW + 7200).to_string();
        assert_eq!(
            header(&reply, "x-codex-primary-reset-at"),
            Some(reset.as_str())
        );
        limit.limit.window = LimitWindow::Weekly;
        let weekly = super::reply(Format::Responses, &limit, 3, NOW);
        assert_eq!(
            header(&weekly, "x-codex-secondary-used-percent"),
            Some("100")
        );
        assert_eq!(
            header(&weekly, "x-codex-secondary-reset-at"),
            Some(reset.as_str())
        );
        assert_eq!(header(&weekly, "x-codex-primary-reset-at"), None);
        limit.limit.resets = None;
        let silent = super::reply(Format::Responses, &limit, 3, NOW);
        assert_eq!(silent.body["error"]["resets_at"], Value::Null);
    }

    #[test]
    fn an_error_inside_a_stream_is_an_error_event_in_the_shape_of_each_format() {
        let (name, data) = stream_event(Format::Messages, &wanted(ErrorKind::Overloaded), 4);
        assert_eq!(name, "error");
        assert_eq!(
            data,
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } })
        );
        let (name, data) = stream_event(Format::Responses, &wanted(ErrorKind::RateLimit), 4);
        assert_eq!(name, "error");
        assert_eq!(data["type"], "error");
        assert_eq!(data["code"], "rate_limit_exceeded");
        assert_eq!(data["sequence_number"], 4);
    }
}
