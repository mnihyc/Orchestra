//! Member-by-member provider reports, independent observation times and removal of supplied secret values.

use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::facts::ProviderFacts;
use orch_provider_contract::limits;
use orch_provider_contract::limits::MODELS_MAX as MODEL_DISCOVERY_MAX_MODELS;
use orch_types::{Json, Time};
use serde_json::{Map, json};
use std::collections::BTreeSet;

/// Every source binding has the same one-MiB report budget.
const REPORT_BYTES: usize = limits::REPORT_MAX_BYTES;
/// Ordinary source strings have this bound; decimal text has its separate precision-preserving bound.
const TEXT_CHARS: usize = limits::TEXT_MAX_CHARS;
/// The balance contract admits at most thirty-two separately named balances.
const BALANCES: usize = limits::BALANCES_MAX;
/// Windows, spend rows and reset credits share the contract's sixty-four-item bound.
const ITEMS: usize = limits::WINDOWS_MAX;

/// A successful source report can retain good members while naming malformed members or list entries.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadReport {
    /// Only members actually told by this read, with connector-owned field times.
    pub facts: Json,
    /// Member paths, without the malformed values or any supplied secret.
    pub notes: Vec<String>,
}

/// A model source distinguishes unsupported from an empty list, and never changes accepted models.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadModels {
    /// None means the source does not list models; Some(empty) means it listed none.
    pub models: Option<Vec<ModelSuggestion>>,
    /// Paths of malformed or excess suggestions omitted from this report.
    pub notes: Vec<String>,
}

fn scrub(text: &str, secrets: &[String]) -> String {
    let mut values: Vec<&str> = secrets
        .iter()
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .collect();
    values.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    let mut text = text.to_owned();
    for value in values {
        text = text.replace(value, "[secret]");
    }
    text
}

/// Redacts longer overlapping secrets first and bounds a source's sentence without cutting UTF-8.
#[must_use]
pub fn scrub_message(message: &str, secrets: &[String]) -> String {
    limits::scrub(
        message,
        &secrets.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

pub(crate) fn scrub_value(value: &Json, secrets: &[String]) -> Json {
    match value {
        Json::String(text) => Json::String(scrub(text, secrets)),
        Json::Array(values) => Json::Array(
            values
                .iter()
                .map(|value| scrub_value(value, secrets))
                .collect(),
        ),
        Json::Object(values) => Json::Object(
            values
                .iter()
                .map(|(name, value)| {
                    let name = if name.starts_with("x_") {
                        scrub(name, secrets)
                    } else {
                        name.clone()
                    };
                    (name, scrub_value(value, secrets))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn text(value: &Json, nonempty: bool) -> bool {
    value
        .as_str()
        .is_some_and(|text| (!nonempty || !text.is_empty()) && text.chars().count() <= TEXT_CHARS)
}

fn optional(value: Option<&Json>, valid: impl FnOnce(&Json) -> bool) -> bool {
    value.is_none_or(|value| value.is_null() || valid(value))
}

fn decimal(value: &Json) -> bool {
    value.as_str().is_some_and(limits::is_decimal)
}

fn time(value: &Json) -> bool {
    value
        .as_str()
        .is_some_and(|value| value.parse::<Time>().is_ok())
}

fn nonnegative(value: &Json) -> bool {
    value
        .as_f64()
        .is_some_and(|value| value.is_finite() && value >= 0.0)
}

fn positive(value: &Json) -> bool {
    value.as_i64().is_some_and(|value| value > 0)
}

fn native(member: &str, value: &Json) -> bool {
    let mut one = Map::new();
    one.insert(member.to_owned(), value.clone());
    serde_json::from_value::<ProviderFacts>(Json::Object(one)).is_ok()
}

fn item(value: &Json, fields: &[&str]) -> Option<Json> {
    Some(Json::Object(
        value
            .as_object()?
            .iter()
            .filter(|(name, _)| fields.contains(&name.as_str()) || name.starts_with("x_"))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    ))
}

fn required(value: &Json, field: &str) -> bool {
    value.get(field).is_some_and(|value| text(value, true))
}

fn strings(value: &Json, fields: &[&str]) -> bool {
    fields
        .iter()
        .all(|field| optional(value.get(*field), |value| text(value, false)))
}

fn amounts(value: &Json, fields: &[&str]) -> bool {
    fields
        .iter()
        .all(|field| optional(value.get(*field), decimal))
}

fn list(
    name: &str,
    value: &Json,
    limit: usize,
    notes: &mut Vec<String>,
    clean: impl Fn(&Json) -> Option<Json>,
) -> Option<Json> {
    if value.is_null() {
        return Some(Json::Null);
    }
    let Some(values) = value.as_array() else {
        notes.push(name.to_owned());
        return None;
    };
    let mut ids = BTreeSet::new();
    let mut kept = Vec::new();
    for (index, value) in values.iter().take(limit).enumerate() {
        let Some(value) = clean(value) else {
            notes.push(format!("{name}[{index}]"));
            continue;
        };
        if let Some(id) = value.get("id").and_then(Json::as_str)
            && !ids.insert(id.to_owned())
        {
            notes.push(format!("{name}[{index}].id"));
            continue;
        }
        kept.push(value);
    }
    if values.len() > limit {
        notes.push(format!("{name}[{limit}]: too_large"));
    }
    Some(Json::Array(kept))
}

fn window(value: &Json) -> Option<Json> {
    let value = item(
        value,
        &[
            "id",
            "label",
            "duration_minutes",
            "used_percent",
            "resets_at",
            "reset_not_started",
            "model_family",
            "is_active",
            "warning_percent",
            "severity",
        ],
    )?;
    if !required(&value, "id")
        || !required(&value, "label")
        || !value.get("used_percent").is_some_and(nonnegative)
        || !optional(value.get("duration_minutes"), positive)
        || !optional(value.get("warning_percent"), |value| {
            nonnegative(value) && value.as_f64().is_some_and(|value| value <= 100.0)
        })
        || !strings(&value, &["model_family", "severity"])
        || !native("windows", &json!([value]))
    {
        return None;
    }
    Some(value)
}

fn balance(value: &Json) -> Option<Json> {
    let value = item(
        value,
        &[
            "id",
            "label",
            "unit",
            "remaining",
            "total",
            "used",
            "unlimited",
            "parts",
            "expires_at",
        ],
    )?;
    if !required(&value, "id")
        || !required(&value, "unit")
        || !strings(&value, &["label"])
        || !amounts(&value, &["remaining", "total", "used"])
        || !optional(value.get("unlimited"), Json::is_boolean)
        || !optional(value.get("expires_at"), time)
        || !optional(value.get("parts"), |parts| {
            parts.as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .all(|part| required(part, "label") && part.get("amount").is_some_and(decimal))
            })
        })
        || !native("balances", &json!([value]))
    {
        return None;
    }
    Some(value)
}

fn spend(value: &Json) -> Option<Json> {
    let value = item(
        value,
        &[
            "id",
            "label",
            "unit",
            "period",
            "period_minutes",
            "used",
            "limit",
            "remaining",
            "resets_at",
        ],
    )?;
    if !required(&value, "id")
        || !required(&value, "unit")
        || !strings(&value, &["label"])
        || !value
            .get("period")
            .and_then(Json::as_str)
            .is_some_and(|period| matches!(period, "day" | "week" | "month" | "total" | "window"))
        || !optional(value.get("period_minutes"), positive)
        || !amounts(&value, &["used", "limit", "remaining"])
        || !optional(value.get("resets_at"), time)
        || !native("spend", &json!([value]))
    {
        return None;
    }
    Some(value)
}

/// Reads facts independently: an absent or malformed member does not acquire another member's value or date.
/// Supplied field times are never trusted, and credential proof survives only a read that used the credential.
pub fn read(
    value: &Json,
    at: Time,
    credential_used: bool,
    secrets: &[String],
) -> Result<ReadReport, &'static str> {
    if serde_json::to_vec(value).map_err(|_| "malformed")?.len() > REPORT_BYTES {
        return Err("too_large");
    }
    let clean = scrub_value(value, secrets);
    let Some(members) = clean.as_object() else {
        return Err("malformed");
    };
    let mut facts = Map::new();
    let mut times = Map::new();
    let mut notes = Vec::new();
    for (name, value) in members {
        if matches!(name.as_str(), "credential_verified" | "credential") && !credential_used {
            continue;
        }
        let kept = match name.as_str() {
            "windows" => list(name, value, ITEMS, &mut notes, window),
            "balances" => list(name, value, BALANCES, &mut notes, balance),
            "spend" => list(name, value, limits::SPEND_MAX, &mut notes, spend),
            "plan" | "identity" => {
                optional(Some(value), |value| text(value, false)).then(|| value.clone())
            }
            "ordinary_usage_allowed" | "credential_verified" => {
                optional(Some(value), Json::is_boolean).then(|| value.clone())
            }
            "available_resets" => {
                if value.is_null() {
                    Some(Json::Null)
                } else {
                    let mut reset = item(value, &["available_count", "credits"]);
                    if let Some(reset) = &mut reset
                        && let Some(credits) = reset.get("credits").cloned()
                    {
                        let credits = list(
                            "available_resets.credits",
                            &credits,
                            limits::RESET_CREDITS_MAX,
                            &mut notes,
                            |credit| {
                                let credit = item(
                                    credit,
                                    &[
                                        "id",
                                        "title",
                                        "description",
                                        "reset_type",
                                        "status",
                                        "granted_at",
                                        "expires_at",
                                    ],
                                )?;
                                (required(&credit, "id")
                                    && strings(
                                        &credit,
                                        &["title", "description", "reset_type", "status"],
                                    )
                                    && native(
                                        "available_resets",
                                        &json!({"available_count":0,"credits":[credit]}),
                                    ))
                                .then_some(credit)
                            },
                        );
                        if let Some(credits) = credits {
                            reset["credits"] = credits;
                        } else if let Some(object) = reset.as_object_mut() {
                            object.remove("credits");
                        }
                    }
                    reset.filter(|reset| {
                        reset
                            .get("available_count")
                            .and_then(Json::as_i64)
                            .is_some_and(|count| count >= 0)
                    })
                }
            }
            "limit_reached" => (value.is_null()
                || (required(value, "type") && native(name, value)))
            .then(|| value.clone()),
            "credential" => (value.is_null()
                || (strings(value, &["method"]) && native(name, value)))
            .then(|| value.clone()),
            "extra_usage" => (value.is_null()
                || (strings(value, &["unit", "disabled_reason"]) && native(name, value)))
            .then(|| value.clone()),
            "plan_credits" | "key_usage" | "key_credits" | "key_usage_supported" => {
                (value.is_null() || native(name, value)).then(|| value.clone())
            }
            _ => continue,
        };
        let Some(kept) = kept else {
            if !notes.contains(name) {
                notes.push(name.clone());
            }
            continue;
        };
        times.insert(name.clone(), json!(at));
        if let Some(items) = kept
            .as_array()
            .or_else(|| kept.get("credits").and_then(Json::as_array))
        {
            for item in items {
                if let Some(id) = item.get("id").and_then(Json::as_str) {
                    times.insert(format!("{name}.{id}"), json!(at));
                }
            }
        }
        facts.insert(name.clone(), kept);
    }
    facts.insert("field_times".to_owned(), Json::Object(times));
    Ok(ReadReport {
        facts: Json::Object(facts),
        notes,
    })
}

/// The same normalizer used by the keeper and the API turns internal legacy inputs into general members.
pub fn normalized(report: &ReadReport) -> Result<ProviderFacts, serde_json::Error> {
    let mut facts: ProviderFacts = serde_json::from_value(report.facts.clone())?;
    orch_agent_contract::facts::normalize(&mut facts);
    Ok(facts)
}

/// Reads the model report's nullable list, supplying only the optional display defaults and keeping its cap.
pub fn models(value: &Json, secrets: &[String]) -> Result<ReadModels, &'static str> {
    if serde_json::to_vec(value).map_err(|_| "malformed")?.len() > REPORT_BYTES {
        return Err("too_large");
    }
    if value.is_null() {
        return Ok(ReadModels {
            models: None,
            notes: Vec::new(),
        });
    }
    let Some(values) = value.as_array() else {
        return Err("malformed");
    };
    let mut notes = Vec::new();
    let mut models = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().take(MODEL_DISCOVERY_MAX_MODELS).enumerate() {
        let value = scrub_value(value, secrets);
        let mut value = item(
            &value,
            &["model_name", "display_name", "parameters", "listed"],
        );
        if let Some(value) = value.as_mut().and_then(Json::as_object_mut)
            && let Some(name) = value.get("model_name").cloned()
        {
            value.entry("display_name").or_insert(name);
            value.entry("parameters").or_insert(json!({}));
            value.entry("listed").or_insert(json!(true));
        }
        let Some(value) = value
            .filter(|value| required(value, "model_name") && strings(value, &["display_name"]))
        else {
            notes.push(format!("models[{index}]"));
            continue;
        };
        match serde_json::from_value::<ModelSuggestion>(value) {
            Ok(model) if seen.insert(model.model_name.clone()) => models.push(model),
            _ => notes.push(format!("models[{index}]")),
        }
    }
    if values.len() > MODEL_DISCOVERY_MAX_MODELS {
        notes.push(format!("models[{MODEL_DISCOVERY_MAX_MODELS}]: too_large"));
    }
    Ok(ReadModels {
        models: Some(models),
        notes,
    })
}
