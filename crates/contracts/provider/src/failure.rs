//! Shared classification of model failures from structured evidence, status and vendor words.

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The evidence that decided a classification, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// A recognized vendor or harness error code or type.
    Structured,
    /// An HTTP status, refined by words where the status is ambiguous.
    Status,
    /// Words naming the cause, rather than advice about what to do next.
    Words,
    /// A transport failed before any HTTP response arrived.
    Transport,
    /// No known evidence was supplied.
    Unknown,
}

/// One classification shared by adapters and direct API callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Classification {
    /// The neutral failure kind.
    pub code: String,
    /// A refusal's cyber, safety or other reason, or unavailable access.
    pub reason: Option<String>,
    /// The strongest evidence that decided the kind.
    pub evidence: Evidence,
}

impl Classification {
    /// Whether repeating the request may help without changing its access.
    #[must_use]
    pub fn retryable(&self) -> bool {
        matches!(
            self.code.as_str(),
            "provider_busy" | "provider_unreachable" | "model_refused"
        )
    }

    /// A short stable name used by diagnostics and recorded fixture tests.
    #[must_use]
    pub fn row(&self) -> &'static str {
        match self.code.as_str() {
            "model_refused" => "model_refused",
            "model_unavailable" if self.reason.as_deref() == Some("access") => "model_access",
            "model_unavailable" => "model_not_found",
            "context_overflow" => "context_window",
            "provider_exhausted" => "plan_limit",
            "provider_unauthorized" => "credential_refused",
            "provider_busy" => "busy",
            "provider_unreachable" => "out_of_reach",
            _ => "unknown",
        }
    }
}

/// What any adapter or protocol actually received, without vendor-specific decisions.
#[derive(Debug, Clone, Default)]
pub struct FailureInput<'a> {
    /// Codes and types, most specific first (body code before wrapper type).
    pub codes: Vec<&'a str>,
    /// The HTTP status, if a response arrived.
    pub status: Option<i64>,
    /// The original message and additional details.
    pub message: &'a str,
    /// Whether the transport itself reported a failure.
    pub transport: bool,
}

fn result(code: &str, reason: Option<&str>, evidence: Evidence) -> Classification {
    Classification {
        code: code.to_owned(),
        reason: reason.map(str::to_owned),
        evidence,
    }
}

fn has(text: &str, words: &[&str]) -> bool {
    words.iter().any(|word| text.contains(word))
}

fn refusal(text: &str, evidence: Evidence) -> Classification {
    let reason = if has(text, &["cyber", "daybreak"]) {
        "cyber"
    } else if has(
        text,
        &[
            "safety",
            "safe",
            "policy",
            "content filter",
            "content_filter",
        ],
    ) {
        "safety"
    } else {
        "other"
    };
    result("model_refused", Some(reason), evidence)
}

/// Specific word families in semantic precedence; retry advice is deliberately absent.
fn words(text: &str) -> Option<Classification> {
    let access_subject = has(
        text,
        &[
            "model",
            "account",
            "organization",
            "organisation",
            "program",
            "programme",
            "tier",
            "daybreak",
            "access",
        ],
    );
    // A model-not-found response may list missing access as an alternative. That does not establish
    // an account or programme denial; retain the unavailable-model cause instead.
    let uncertain_access = text.contains(" or ")
        && text.contains("model")
        && has(text, &["does not exist", "doesn't exist", "not found"]);
    if access_subject
        && !uncertain_access
        && has(
            text,
            &[
                "not available for",
                "isn't available for",
                "is not available for",
                "not enabled",
                "not granted",
                "not eligible",
                "does not have access",
                "do not have access",
                "don't have access",
                "no access to",
                "not have permission",
                "not permitted to use",
                "permission to use",
                "requires usage credits",
                "credits required",
                "tier is not allowed",
                "tier is not supported",
            ],
        )
    {
        return Some(result("model_unavailable", Some("access"), Evidence::Words));
    }
    if has(
        text,
        &[
            "cyber",
            "safety",
            "policy",
            "flagged",
            "content filter",
            "content_filter",
            "content-filter",
            "not allowed",
            "rephrase",
            "prompt blocked",
            "request blocked",
        ],
    ) {
        return Some(refusal(text, Evidence::Words));
    }
    if has(
        text,
        &[
            "context length",
            "context_length",
            "context window",
            "context_window",
            "prompt too long",
            "prompt is too long",
            "input too long",
            "maximum length",
            "maximum context",
            "too many tokens",
            "request too large",
            "request is too large",
        ],
    ) {
        return Some(result("context_overflow", None, Evidence::Words));
    }
    if has(
        text,
        &[
            "model not found",
            "model_not_found",
            "unknown model",
            "model does not exist",
            "model unavailable",
            "model is unavailable",
            "model is not available",
            "model has been retired",
        ],
    ) || text.contains("model") && has(text, &["does not exist", "doesn't exist"])
    {
        return Some(result("model_unavailable", None, Evidence::Words));
    }
    // A denial of a usage limit is not evidence of one; rate limits are handled below.
    if !has(
        text,
        &[
            "not your usage limit",
            "not a usage limit",
            "not your quota",
        ],
    ) && (has(
        text,
        &[
            "quota",
            "usage limit",
            "usage_limit",
            "usage credits",
            "credit balance",
            "credits exhausted",
            "out of credits",
            "insufficient credits",
            "no balance",
            "billing",
            "spend limit",
            "spending limit",
            "monthly limit",
            "weekly limit",
            "session limit",
            "hit your limit",
        ],
    ) || text.contains("limit") && text.contains("reset") && !text.contains("rate limit"))
    {
        return Some(result("provider_exhausted", None, Evidence::Words));
    }
    let authentication_denied = text
        .split(['.', '!', '?', ';', ':', ',', '\n', '\u{b7}'])
        .any(|clause| {
            let token = clause
                .split(|character: char| !character.is_alphanumeric())
                .any(|word| word == "token" || word == "tokens");
            // Token throughput is not a credential. A request can be rejected for its token rate
            // while the key itself remains valid for every user of the provider.
            let token_usage = has(
                clause,
                &[
                    "token rate",
                    "token limit",
                    "token budget",
                    "token usage",
                    "token count",
                    "input token",
                    "output token",
                    "request tokens",
                    "number of tokens",
                ],
            );
            let subject = has(
                clause,
                &[
                    "api key",
                    "apikey",
                    "authentication",
                    "authenticate",
                    "credential",
                    "sign in",
                    "signin",
                    "log in",
                    "login",
                ],
            ) || token && !token_usage;
            let denied = has(
                clause,
                &[
                    "invalid",
                    "incorrect",
                    "expired",
                    "revoked",
                    "rejected",
                    "refused",
                    "denied",
                    "missing",
                    "require",
                    "failed",
                    "failure",
                    "error",
                    "not found",
                    "not signed in",
                    "not logged in",
                    "must sign in",
                    "must log in",
                    "need to sign in",
                    "need to log in",
                    "no credential",
                    "no api key",
                ],
            );
            let negated = has(
                clause,
                &[
                    "not invalid",
                    "not expired",
                    "not revoked",
                    "not rejected",
                    "not missing",
                    "not an authentication",
                    "not a credential",
                ],
            );
            subject && denied && !negated
        });
    if authentication_denied || has(text, &["unauthorized", "unauthorised", "unauthenticated"]) {
        return Some(result("provider_unauthorized", None, Evidence::Words));
    }
    if has(
        text,
        &[
            "overload",
            "at capacity",
            "capacity reached",
            "rate limit",
            "rate_limit",
            "too many requests",
            "throttl",
            "server busy",
            "service busy",
            "temporarily limiting",
            "server error",
            "server_error",
        ],
    ) {
        return Some(result("provider_busy", None, Evidence::Words));
    }
    if has(
        text,
        &[
            "timeout",
            "timed out",
            "econnreset",
            "econnrefused",
            "connection reset",
            "connection was reset",
            "connection refused",
            "connection failed",
            "connection lost",
            "connection to the api was lost",
            "stream disconnected",
            "dns",
            "tls",
            "certificate",
            "network error",
            "no response",
            "no_response",
            "malformed response",
            "empty response",
            "streamnoeventserror",
            "0 stream events",
            "could not resolve",
            "name resolution",
        ],
    ) || has(text, &["api error", "api response"])
        && has(text, &["json parse", "json parsing", "unterminated string"])
    {
        // An unreadable API envelope is a broken response. A model's invalid JSON output alone
        // does not establish a transport failure.
        return Some(result("provider_unreachable", None, Evidence::Words));
    }
    if has(
        text,
        &["sign in", "signin", "authenticate", "log in", "login"],
    ) {
        return Some(result("provider_unauthorized", None, Evidence::Words));
    }
    None
}

/// Classifies only evidence supplied by the caller, preserving unknown failures as harness errors.
#[must_use]
pub fn classify(input: &FailureInput<'_>) -> Classification {
    let text = input
        .message
        .to_lowercase()
        .replace('\u{2019}', "'")
        .replace(['-', '_'], " ");
    // Generic wrapper types describe several causes. They defer to a more specific code or words.
    let mut generic = None;
    let mut server_wrapper = false;
    for code in &input.codes {
        let code = code.to_ascii_lowercase();
        let explicit = match code.as_str() {
            "model_access_denied"
            | "access_not_granted"
            | "model_requires_usage_credits"
            | "long_context_credits_required"
            | "account_on_hold"
            | "verification_required" => Some(("model_unavailable", Some("access"))),
            "content_policy_violation"
            | "content_filter"
            | "safety_violation"
            | "model_refused"
            | "refusal" => return refusal(&format!("{code} {text}"), Evidence::Structured),
            "contextwindowexceeded"
            | "context_length_exceeded"
            | "prompt_too_long"
            | "image_error"
            | "request_too_large" => Some(("context_overflow", None)),
            "model_not_found" | "model_unavailable" => Some(("model_unavailable", None)),
            "usagelimitexceeded"
            | "usage_limit_reached"
            | "insufficient_quota"
            | "credit_balance_exhausted"
            | "organization_spend_limit_exceeded"
            | "project_spend_limit_exceeded"
            | "organization_usage_limit_exceeded"
            | "billing_error"
            | "billing_hard_limit_reached" => Some(("provider_exhausted", None)),
            "authentication_failed" if input.status == Some(403) => {
                // Some harnesses wrap every forbidden resource as an authentication failure.
                // Specific vendor words distinguish model policy from a rejected key.
                generic = Some("provider_unauthorized");
                None
            }
            "unauthorized"
            | "authentication_error"
            | "authentication_failed"
            | "invalid_api_key"
            | "invalid_authentication"
            | "provider_credentials"
            | "gateway_signin_required"
            | "gateway_session_expired"
            | "api_key_auth_disabled"
            | "org_disabled_credential"
            | "invalid_credential_header"
            | "oauth_org_not_allowed"
            | "cloud_credential_error" => Some(("provider_unauthorized", None)),
            "serveroverloaded"
            | "internalservererror"
            | "flexunavailable"
            | "overloaded_error"
            | "server_is_overloaded"
            | "service_unavailable_error"
            | "overloaded" => Some(("provider_busy", None)),
            "server_error" | "api_error" => {
                server_wrapper = true;
                None
            }
            "timeout_error" | "no_response" | "tls_untrusted_ca" | "gateway_content_type" => {
                Some(("provider_unreachable", None))
            }
            "ratelimitexceeded"
            | "rate_limit_error"
            | "rate_limit_exceeded"
            | "rate_limit"
            | "slow_down" => {
                generic = Some("provider_busy");
                None
            }
            "permission_error" => {
                generic = Some("model_refused");
                None
            }
            _ => None,
        };
        if let Some((kind, reason)) = explicit {
            return result(kind, reason, Evidence::Structured);
        }
    }
    let by_words = words(&text);
    // Ambiguous request statuses and rate-limit wrappers may name access, refusal or quota in words.
    if matches!(input.status, None | Some(200..=299 | 400 | 403 | 404 | 429))
        && let Some(found) = by_words.as_ref()
        && !matches!(
            found.code.as_str(),
            "provider_busy" | "provider_unreachable"
        )
    {
        return found.clone();
    }
    let status_kind = match input.status {
        Some(401) => Some("provider_unauthorized"),
        Some(402) => Some("provider_exhausted"),
        Some(403) => return refusal(&text, Evidence::Status),
        Some(404) if text.contains("model") => Some("model_unavailable"),
        Some(408 | 504) => Some("provider_unreachable"),
        Some(413) => Some("context_overflow"),
        Some(429 | 500..=599) => Some("provider_busy"),
        _ => None,
    };
    if let Some(kind) = status_kind {
        return result(kind, None, Evidence::Status);
    }
    if let Some(kind) = generic {
        return if kind == "model_refused" {
            refusal(&text, Evidence::Structured)
        } else {
            result(kind, None, Evidence::Structured)
        };
    }
    if let Some(found) = by_words {
        return found;
    }
    if input.transport {
        return result("provider_unreachable", None, Evidence::Transport);
    }
    if server_wrapper {
        return result("provider_busy", None, Evidence::Structured);
    }
    // Retry advice alone names no cause and cannot make an unknown failure transient.
    result("harness_error", None, Evidence::Unknown)
}

/// Classifies a protocol error body using its code, type, status and original message.
#[must_use]
pub fn classify_body(status: Option<i64>, body: &Json, message: &str) -> Classification {
    let error = body.get("error").unwrap_or(body);
    classify(&FailureInput {
        codes: ["code", "type"]
            .into_iter()
            .filter_map(|key| error.get(key).and_then(Json::as_str))
            .collect(),
        status,
        message,
        transport: false,
    })
}

/// Reads documented refusal fields in successful Responses, Chat and Messages replies.
#[must_use]
pub fn completed_refusal(body: &Json) -> Option<(Classification, String)> {
    let chat = body
        .get("choices")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .find_map(|choice| {
            choice
                .pointer("/message/refusal")
                .and_then(Json::as_str)
                .filter(|text| !text.is_empty())
        });
    let response = body
        .get("output")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| {
            item.get("content")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
        })
        .find(|part| part["type"] == "refusal");
    let message = body["stop_reason"] == "refusal";
    let filtered = body
        .get("choices")
        .and_then(Json::as_array)
        .is_some_and(|choices| {
            choices
                .iter()
                .any(|choice| choice["finish_reason"] == "content_filter")
        })
        || body
            .pointer("/incomplete_details/reason")
            .and_then(Json::as_str)
            == Some("content_filter");
    if chat.is_none() && response.is_none() && !message && !filtered {
        return None;
    }
    let words = chat
        .map(str::to_owned)
        .or_else(|| {
            response
                .and_then(|part| part["refusal"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            body.get("content")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
    let mut classified = classify(&FailureInput {
        codes: vec!["content_filter"],
        status: None,
        message: &words,
        transport: false,
    });
    if body
        .pointer("/stop_details/category")
        .and_then(Json::as_str)
        .is_some_and(|category| category.to_lowercase().contains("cyber"))
    {
        classified.reason = Some("cyber".to_owned());
    }
    Some((
        classified,
        if words.is_empty() {
            body.to_string()
        } else {
            words
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        codes: Vec<String>,
        status: Option<i64>,
        message: String,
        expected: String,
        reason: Option<String>,
        why: String,
    }

    #[test]
    fn documented_recorded_and_conflicting_failures_have_one_meaning() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("failure_cases.json")).unwrap();
        for case in cases {
            let actual = classify(&FailureInput {
                codes: case.codes.iter().map(String::as_str).collect(),
                status: case.status,
                message: &case.message,
                transport: false,
            });
            assert_eq!(
                (actual.code, actual.reason),
                (case.expected, case.reason),
                "{}: {}",
                case.name,
                case.why
            );
        }
    }

    #[test]
    fn successful_refusal_envelopes_are_failures_but_ordinary_answers_are_not_classified_by_their_words()
     {
        for body in [
            serde_json::json!({"choices":[{"message":{"refusal":"Safety policy"}}]}),
            serde_json::json!({"output":[{"type":"message","content":[{"type":"refusal","refusal":"Safety policy"}]}]}),
            serde_json::json!({"stop_reason":"refusal","stop_details":{"category":"cyber"},"content":[{"type":"text","text":"Request declined"}]}),
        ] {
            assert_eq!(completed_refusal(&body).unwrap().0.code, "model_refused");
        }
        assert!(completed_refusal(&serde_json::json!({"output":[{"type":"message","content":[{"type":"output_text","text":"We discuss cyber safety"}]}]})).is_none());
    }
}

#[cfg(test)]
mod filter_stops {
    use super::*;
    use serde_json::json;

    #[test]
    #[allow(clippy::expect_used, reason = "a test fails by panicking")]
    fn documented_filter_stops_are_refusals_without_a_refusal_text_field() {
        for body in [
            json!({"choices":[{"finish_reason":"content_filter","message":{"content":null}}]}),
            json!({"status":"incomplete","incomplete_details":{"reason":"content_filter"},"output":[]}),
        ] {
            let (failure, _) =
                completed_refusal(&body).expect("the explicit filter stop is a refusal");
            assert_eq!(failure.code, "model_refused");
            assert_eq!(failure.reason.as_deref(), Some("safety"));
        }
        for body in [
            json!({"choices":[{"finish_reason":"length","message":{"content":"Some output"}}]}),
            json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}),
        ] {
            assert!(
                completed_refusal(&body).is_none(),
                "output bounds are not filter refusals"
            );
        }
    }
}
