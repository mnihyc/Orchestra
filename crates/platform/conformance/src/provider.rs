//! The suite every provider source passes: declarations, member states, models, resets, presentation, quiet errors and bounds.
//!
//! Fixtures describe a disposable account and its exact controlled answers. Resets are spent only
//! when the caller explicitly supplies a reset fixture; discovery never authorizes spending one.
//! A missing fixture or an unsupported element is skipped, never reported as a passed exercise.

use std::future::Future;
use std::time::Duration;

use orch_provider_contract::limits::{
    BALANCES_MAX as BALANCES, REPORT_MAX_BYTES as REPLY_BYTES, RESET_CREDITS_MAX as RESETS,
    SPEND_MAX as SPEND, TEXT_MAX_CHARS as TEXT_CHARS, WINDOWS_MAX as WINDOWS,
};
use orch_provider_contract::source::{
    CredentialPresenter, FactsSource, ModelsSource, PresentAsk, ResetRequest, SourceAsk,
    SourceDescriptor, SourceError,
};
use orch_provider_contract::wire::{LIST_DEADLINE as LONG_READ, READ_DEADLINE as SHORT_READ};
use orch_types::{Json, Time};
use serde::{Deserialize, Serialize};

use crate::report::{Checked, Report, skip};

/// The implementations of a source's independently optional elements.
pub struct Sources<'a> {
    /// Its common declaration, checked before any source method is exercised.
    pub descriptor: &'a SourceDescriptor,
    /// Facts and spending a reset, when declared.
    pub facts: Option<&'a dyn FactsSource>,
    /// Models, when declared.
    pub models: Option<&'a dyn ModelsSource>,
    /// Keys-only credential presentation, never supplied for an address binding.
    pub presenter: Option<&'a dyn CredentialPresenter>,
}

/// One controlled facts or models answer, retaining absent members and explicit nulls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    /// Parameters for this disposable account or controlled answer mode.
    pub ask: SourceAsk,
    /// The exact report on the wire, before filling any absent member with a default.
    pub expected: Json,
}

/// Explicit permission to spend one disposable reset with one replay key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetExample {
    /// The disposable account whose count is checked before, between and after the two calls.
    pub ask: SourceAsk,
    /// The exact offered reset and one idempotency key, used for both calls.
    pub reset: ResetRequest,
}

/// A controlled key exchange, with an optional exact result in addition to header/expiry checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresentExample {
    /// A disposable example key and endpoint, never a sign-in credential.
    pub ask: PresentAsk,
    /// The exact answer where a fixed-clock counterpart can supply it.
    pub expected: Option<Json>,
}

/// Parameters which the controlled source refuses, to check that its errors keep secrets quiet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefusalExample {
    /// A refused facts/models read carrying disposable secret fields.
    pub ask: SourceAsk,
    /// A refused presentation, where that element is implemented.
    pub present: Option<PresentAsk>,
}

/// Controlled member-state examples and explicitly authorized effects, supplied by the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// Facts cases covering an absent member, null, an empty list and a value.
    #[serde(default)]
    pub facts: Vec<Example>,
    /// Models cases covering null, an empty list and a nonempty list.
    #[serde(default)]
    pub models: Vec<Example>,
    /// Omit this unless spending the named reset on a disposable account is intended.
    pub reset: Option<ResetExample>,
    /// Omit this unless a disposable key exchange is available.
    pub present: Option<PresentExample>,
    /// An explicitly controlled refusal; normal reads cannot prove error redaction.
    pub refusal: Option<RefusalExample>,
    /// The controlled clock against which a presenter's expiry is checked.
    pub now: Time,
    /// Additional disposable secrets, such as an address's bearer token, absent from SourceAsk.
    #[serde(default)]
    pub secrets: Vec<String>,
}

#[derive(Default)]
struct Bounds {
    calls: usize,
    failures: Vec<String>,
}

/// Checks one source through the same traits for built-ins, programs and addresses.
pub async fn run(mut source: Sources<'_>, fixture: Option<&Fixture>, report: &mut Report) {
    let descriptor = serde_json::to_value(source.descriptor);
    let mut declaration = source
        .descriptor
        .check()
        .map_err(|fault| fault.to_string().into());
    if descriptor.is_err() {
        declaration = Err("the descriptor cannot be written".into());
    }
    if let Ok(value) = &descriptor {
        let claims = |member: &str| value.get(member).is_some_and(|value| !value.is_null());
        if claims("facts") && source.facts.is_none()
            || claims("models") && source.models.is_none()
            || claims("presents") && source.presenter.is_none()
        {
            declaration = Err("a declared element has no supplied trait implementation".into());
        }
        let mut actual = Vec::new();
        if let Some(part) = source.facts {
            actual.push(part.describe());
        }
        if let Some(part) = source.models {
            actual.push(part.describe());
        }
        if let Some(part) = source.presenter {
            actual.push(part.describe());
        }
        if actual
            .iter()
            .any(|part| serde_json::to_value(part).as_ref().ok() != Some(value))
        {
            declaration = Err("the source's element declarations disagree".into());
        }
        if !claims("facts") {
            source.facts = None;
        }
        if !claims("models") {
            source.models = None;
        }
        if !claims("presents") {
            source.presenter = None;
        }
    }
    report.checked(
        "provider.descriptor",
        "Its declaration names the optional elements and usable fields",
        declaration,
    );
    let Some(fixture) = fixture else {
        for rule in [
            "provider.facts.shape",
            "provider.facts.states",
            "provider.models",
            "provider.resets.idempotent",
            "provider.credential.present",
            "provider.quiet",
            "provider.bounds",
        ] {
            report.checked(
                rule,
                "Its controlled behavior is exercised",
                Err(skip("supply a disposable provider fixture")),
            );
        }
        return;
    };
    let mut secrets = fixture.secrets.clone();
    for case in fixture.facts.iter().chain(&fixture.models) {
        ask_secrets(&case.ask, &mut secrets);
    }
    if let Some(case) = &fixture.reset {
        ask_secrets(&case.ask, &mut secrets);
    }
    if let Some(case) = &fixture.present {
        ask_secrets(&case.ask, &mut secrets);
    }
    if let Some(case) = &fixture.refusal {
        ask_secrets(&case.ask, &mut secrets);
        if let Some(ask) = &case.present {
            ask_secrets(ask, &mut secrets);
        }
    }
    secrets.retain(|secret| !secret.is_empty());
    secrets.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    secrets.dedup();
    let mut bounds = Bounds::default();
    let mut facts = Vec::new();
    match source.facts {
        Some(source) if !fixture.facts.is_empty() => {
            let mut shape = Ok(());
            let mut states = Ok(());
            for case in &fixture.facts {
                match measured(
                    "facts.read",
                    SHORT_READ,
                    source.read_facts(case.ask.clone()),
                    &secrets,
                    &mut bounds,
                )
                .await
                {
                    Ok(value) => {
                        if let Err(error) = facts_shape(&value) {
                            shape = Err(error.into());
                        }
                        if value != case.expected {
                            states=Err("a controlled facts report lost presence, null, emptiness or a value".into());
                        }
                        facts.push(value);
                    }
                    Err(error) => {
                        shape = Err(error.clone().into());
                        states = Err(error.into());
                    }
                }
            }
            report.checked(
                "provider.facts.shape",
                "Facts retain their types, units, decimal amounts and item bounds",
                shape,
            );
            if states.is_ok() && !four_states(&facts) {
                states = Err(skip(
                    "facts fixtures must exercise absent, null, empty-list and value separately",
                ));
            }
            report.checked(
                "provider.facts.states",
                "Absent, null, empty lists and values stay distinct",
                states,
            );
        }
        _ => {
            for rule in ["provider.facts.shape", "provider.facts.states"] {
                report.checked(
                    rule,
                    "Facts are exercised",
                    Err(skip(
                        "no facts implementation or controlled facts cases supplied",
                    )),
                );
            }
        }
    }
    match source.models {
        Some(source) if !fixture.models.is_empty() => {
            let mut checked = Ok(());
            let mut kinds = [false; 3];
            for case in &fixture.models {
                match measured(
                    "models.list",
                    LONG_READ,
                    source.list_models(case.ask.clone()),
                    &secrets,
                    &mut bounds,
                )
                .await
                {
                    Ok(value) => {
                        if let Err(error) = models_shape(&value) {
                            checked = Err(error.into());
                        }
                        if value != case.expected {
                            checked=Err("the controlled models report differs or collapsed unsupported and empty".into());
                        }
                        match value.get("models") {
                            Some(Json::Null) => kinds[0] = true,
                            Some(Json::Array(rows)) if rows.is_empty() => kinds[1] = true,
                            Some(Json::Array(_)) => kinds[2] = true,
                            _ => {}
                        }
                    }
                    Err(error) => checked = Err(error.into()),
                }
            }
            if checked.is_ok() && !kinds.into_iter().all(|seen| seen) {
                checked = Err(skip(
                    "models fixtures must exercise null, empty and nonempty lists",
                ));
            }
            report.checked(
                "provider.models",
                "Unsupported, empty and offered model lists remain distinct",
                checked,
            );
        }
        _ => report.checked(
            "provider.models",
            "Models are exercised",
            Err(skip(
                "no models implementation or controlled cases supplied",
            )),
        ),
    }
    match (source.facts.filter(|_|source.descriptor.spends_resets()),fixture.reset.as_ref()) {
        (Some(source),Some(case))=>{
            let checked=reset_case(source,case,&secrets,&mut bounds).await;
            report.checked("provider.resets.idempotent","One explicit reset replay spends one credit",checked);
        },
        _=>report.checked("provider.resets.idempotent","A disposable reset is explicitly spent",Err(skip("the source does not spend resets, or no explicit disposable reset fixture was supplied"))),
    }
    match (source.presenter, fixture.present.as_ref()) {
        (Some(source), Some(case)) => {
            let checked = match measured(
                "credential.present",
                SHORT_READ,
                source.present(case.ask.clone()),
                &secrets,
                &mut bounds,
            )
            .await
            {
                Ok(value) => present_shape(&value, fixture.now)
                    .and_then(|()| {
                        if case
                            .expected
                            .as_ref()
                            .is_some_and(|expected| expected != &value)
                        {
                            Err(
                                "the controlled token exchange differs from its expected result"
                                    .into(),
                            )
                        } else {
                            Ok(())
                        }
                    })
                    .map_err(Into::into),
                Err(error) => Err(error.into()),
            };
            report.checked(
                "provider.credential.present",
                "Presented headers are usable and any expiry is in the future",
                checked,
            );
        }
        _ => report.checked(
            "provider.credential.present",
            "A disposable key is presented",
            Err(skip(
                "no presenter (addresses never present) or disposable key fixture supplied",
            )),
        ),
    }
    if let Some(case) = &fixture.refusal {
        let mut checked = Ok(());
        let mut tested = false;
        if let Some(source) = source.facts {
            tested = true;
            checked = quiet_measured(
                "facts.read",
                SHORT_READ,
                source.read_facts(case.ask.clone()),
                &secrets,
                &mut bounds,
            )
            .await;
        }
        if let Some(source) = source.models {
            tested = true;
            if let Err(error) = quiet_measured(
                "models.list",
                LONG_READ,
                source.list_models(case.ask.clone()),
                &secrets,
                &mut bounds,
            )
            .await
            {
                checked = Err(error);
            }
        }
        if let (Some(source), Some(ask)) = (source.presenter, case.present.as_ref()) {
            tested = true;
            if let Err(error) = quiet_measured(
                "credential.present",
                SHORT_READ,
                source.present(ask.clone()),
                &secrets,
                &mut bounds,
            )
            .await
            {
                checked = Err(error);
            }
        }
        if !tested {
            checked = Err(skip("no implemented element has a controlled refusal"));
        }
        report.checked(
            "provider.quiet",
            "No secret given to the source appears in its refusal",
            checked,
        );
    } else {
        report.checked(
            "provider.quiet",
            "Errors keep secrets quiet",
            Err(skip(
                "supply a controlled refusal carrying disposable secrets",
            )),
        );
    }
    report.checked(
        "provider.bounds",
        "Every exercised method stays within its deadline and reply size",
        if bounds.calls == 0 {
            Err(skip("no source methods exercised"))
        } else if bounds.failures.is_empty() {
            Ok(())
        } else {
            Err(bounds.failures.join("; ").into())
        },
    );
}

async fn measured<T: Serialize>(
    method: &str,
    within: Duration,
    call: impl Future<Output = Result<T, SourceError>>,
    secrets: &[String],
    bounds: &mut Bounds,
) -> Result<Json, String> {
    bounds.calls += 1;
    match tokio::time::timeout(within, call).await {
        Err(_) => {
            let error = format!("{method} exceeded its deadline");
            bounds.failures.push(error.clone());
            Err(error)
        }
        Ok(Err(error)) => {
            if matches!(error.code.as_str(), "timeout" | "too_large") {
                bounds
                    .failures
                    .push(format!("{method} exceeded its {} bound", error.code));
            }
            if error.message.chars().count()
                > orch_provider_contract::limits::ERROR_MESSAGE_MAX_CHARS
            {
                bounds
                    .failures
                    .push(format!("{method} exceeded the error-message bound"));
            }
            Err(scrub(error.to_string(), secrets))
        }
        Ok(Ok(result)) => {
            let value = serde_json::to_value(result)
                .map_err(|_| format!("{method} returned an unwritable report"))?;
            let bytes = serde_json::to_vec(&value)
                .map_err(|_| format!("{method} returned an unwritable report"))?;
            if bytes.len() > REPLY_BYTES {
                let error = format!("{method} exceeded the reply size");
                bounds.failures.push(error.clone());
                return Err(error);
            }
            Ok(value)
        }
    }
}

async fn reset_case(
    source: &dyn FactsSource,
    case: &ResetExample,
    secrets: &[String],
    bounds: &mut Bounds,
) -> Checked {
    let before = measured(
        "facts.read",
        SHORT_READ,
        source.read_facts(case.ask.clone()),
        secrets,
        bounds,
    )
    .await?;
    let count = reset_count(&before).ok_or("the reset fixture has no available reset count")?;
    if count <= 0 {
        return Err(skip(
            "the disposable reset is unavailable; nothing was spent",
        ));
    }
    let offered = before
        .pointer("/available_resets/credits")
        .and_then(Json::as_array)
        .is_some_and(|credits| {
            credits.iter().any(|credit| {
                credit.get("id").and_then(Json::as_str) == Some(case.reset.reset_id.as_str())
                    && credit.get("status").and_then(Json::as_str) == Some("available")
            })
        });
    if !offered {
        return Err(
            "the explicit fixture reset is not listed as available; nothing was spent".into(),
        );
    }
    let first = measured(
        "resets.spend",
        LONG_READ,
        source.spend_reset(case.ask.clone(), case.reset.clone()),
        secrets,
        bounds,
    )
    .await?;
    let middle = measured(
        "facts.read",
        SHORT_READ,
        source.read_facts(case.ask.clone()),
        secrets,
        bounds,
    )
    .await?;
    let second = measured(
        "resets.spend",
        LONG_READ,
        source.spend_reset(case.ask.clone(), case.reset.clone()),
        secrets,
        bounds,
    )
    .await?;
    let after = measured(
        "facts.read",
        SHORT_READ,
        source.read_facts(case.ask.clone()),
        secrets,
        bounds,
    )
    .await?;
    // A source may read fresh facts or say "already spent" on the replay. Idempotence concerns
    // the effect, not byte-identical snapshots while the vendor's account keeps changing.
    for reply in [&first, &second] {
        required_text(reply, "outcome")?;
        if let Some(facts) = reply.get("facts").filter(|facts| !facts.is_null()) {
            facts_shape(facts)?;
        }
    }
    if reset_count(&middle) != Some(count - 1) || reset_count(&after) != reset_count(&middle) {
        Err("the reset replay did not spend exactly once".into())
    } else {
        Ok(())
    }
}

fn reset_count(value: &Json) -> Option<i64> {
    value
        .get("available_resets")?
        .get("available_count")?
        .as_i64()
}

async fn quiet_measured<T>(
    method: &str,
    within: Duration,
    call: impl Future<Output = Result<T, SourceError>>,
    secrets: &[String],
    bounds: &mut Bounds,
) -> Checked {
    bounds.calls += 1;
    match tokio::time::timeout(within, call).await {
        Ok(answer) => {
            if let Err(error) = &answer
                && matches!(error.code.as_str(), "timeout" | "too_large")
            {
                bounds
                    .failures
                    .push(format!("{method} exceeded its {} bound", error.code));
            }
            if let Err(error) = &answer
                && error.message.chars().count()
                    > orch_provider_contract::limits::ERROR_MESSAGE_MAX_CHARS
            {
                bounds
                    .failures
                    .push(format!("{method} exceeded the error-message bound"));
            }
            quiet(answer.map(|_| ()), secrets)
        }
        Err(_) => {
            let error = format!("{method} exceeded its deadline");
            bounds.failures.push(error.clone());
            Err(error.into())
        }
    }
}

fn quiet(answer: Result<(), SourceError>, secrets: &[String]) -> Checked {
    match answer {
        Ok(()) => {
            Err("the controlled refusal was accepted; no error redaction was exercised".into())
        }
        Err(error) => {
            let text = error.to_string();
            if secrets
                .iter()
                .any(|secret| !secret.is_empty() && text.contains(secret))
            {
                Err("the refusal exposed a secret (value withheld)".into())
            } else {
                Ok(())
            }
        }
    }
}

fn scrub(text: String, secrets: &[String]) -> String {
    orch_provider_contract::limits::scrub(
        &text,
        &secrets.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

fn ask_secrets(ask: &impl Serialize, secrets: &mut Vec<String>) {
    fn strings(value: &Json, secrets: &mut Vec<String>) {
        match value {
            Json::String(text) => secrets.push(text.clone()),
            Json::Array(values) => {
                for value in values {
                    strings(value, secrets);
                }
            }
            Json::Object(values) => {
                for value in values.values() {
                    strings(value, secrets);
                }
            }
            _ => {}
        }
    }
    if let Ok(value) = serde_json::to_value(ask) {
        for key in ["secrets", "credential"] {
            if let Some(value) = value.get(key) {
                strings(value, secrets);
            }
        }
    }
}

fn four_states(reports: &[Json]) -> bool {
    ["windows", "balances", "spend"].into_iter().any(|member| {
        let mut states = [false; 4];
        for report in reports {
            match report.get(member) {
                None => states[0] = true,
                Some(Json::Null) => states[1] = true,
                Some(Json::Array(rows)) if rows.is_empty() => states[2] = true,
                Some(Json::Array(_)) => states[3] = true,
                _ => {}
            }
        }
        states.into_iter().all(|seen| seen)
    })
}

fn required_text(value: &Json, member: &str) -> Result<(), String> {
    if value
        .get(member)
        .and_then(Json::as_str)
        .is_none_or(|text| text.is_empty() || text.chars().count() > TEXT_CHARS)
    {
        Err(format!(
            "{member} must be nonempty text within the text bound"
        ))
    } else {
        Ok(())
    }
}

fn time(value: &Json) -> bool {
    value.as_str().is_some_and(|text| {
        text.parse::<Time>().is_ok() && (text.ends_with('Z') || text.ends_with("+00:00"))
    })
}

fn decimal(value: &Json) -> bool {
    value
        .as_str()
        .is_some_and(orch_provider_contract::limits::is_decimal)
}

fn list<'a>(value: &'a Json, name: &str, max: usize) -> Result<Option<&'a Vec<Json>>, String> {
    match value.get(name) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Array(rows)) if rows.len() <= max => Ok(Some(rows)),
        _ => Err(format!("{name} must be a list within its item bound")),
    }
}

fn facts_shape(value: &Json) -> Result<(), String> {
    if !value.is_object() {
        return Err("facts must be an object retaining member presence".into());
    }
    // Validate each told value against the canonical facts type without serializing that
    // default-filled type back over the original presence-preserving report.
    for member in orch_provider_contract::members::FactMember::ALL {
        if let Some(told) = value.get(member.as_str()).filter(|value| !value.is_null()) {
            let one = serde_json::Map::from_iter([(member.as_str().to_owned(), told.clone())]);
            let _: orch_agent_contract::facts::ProviderFacts =
                serde_json::from_value(Json::Object(one))
                    .map_err(|_| format!("{} has another contract shape", member.as_str()))?;
        }
    }
    for member in ["plan", "identity"] {
        if value.get(member).is_some_and(|value| !value.is_null()) {
            required_text(value, member)?;
        }
    }
    for member in ["ordinary_usage_allowed", "credential_verified"] {
        if value
            .get(member)
            .is_some_and(|value| !value.is_null() && !value.is_boolean())
        {
            return Err(format!("{member} must be boolean or null"));
        }
    }
    if let Some(rows) = list(value, "windows", WINDOWS)? {
        for row in rows {
            required_text(row, "id")?;
            required_text(row, "label")?;
            if row
                .get("used_percent")
                .and_then(Json::as_f64)
                .is_none_or(|percent| !percent.is_finite() || !(0.0..=100.0).contains(&percent))
            {
                return Err("windows.used_percent must be 0 to 100".into());
            }
            optional_time(row, "resets_at")?;
            for member in ["is_active", "reset_not_started"] {
                optional_bool(row, member)?;
            }
            optional_positive(row, "duration_minutes")?;
            optional_text(row, "model_family")?;
            optional_text(row, "severity")?;
            if row.get("warning_percent").is_some_and(|value| {
                !value.is_null()
                    && value
                        .as_f64()
                        .is_none_or(|percent| !(0.0..=100.0).contains(&percent))
            }) {
                return Err("windows.warning_percent must be 0 to 100".into());
            }
        }
    }
    for (member, max) in [("balances", BALANCES), ("spend", SPEND)] {
        if let Some(rows) = list(value, member, max)? {
            for row in rows {
                required_text(row, "id")?;
                required_text(row, "unit")?;
                optional_text(row, "label")?;
                optional_bool(row, "unlimited")?;
                optional_positive(row, "period_minutes")?;
                for amount in ["remaining", "total", "used", "limit"] {
                    if row
                        .get(amount)
                        .is_some_and(|value| !value.is_null() && !decimal(value))
                    {
                        return Err(format!("{member}.{amount} must be bounded decimal text"));
                    }
                }
                if member == "spend"
                    && !matches!(
                        row.get("period").and_then(Json::as_str),
                        Some("day" | "week" | "month" | "total" | "window")
                    )
                {
                    return Err("spend.period must name its unit of time".into());
                }
                optional_time(row, "resets_at")?;
                optional_time(row, "expires_at")?;
                if let Some(parts) = row.get("parts").filter(|value| !value.is_null()) {
                    let parts = parts.as_array().ok_or("balances.parts must be a list")?;
                    for part in parts {
                        required_text(part, "label")?;
                        if !part.get("amount").is_some_and(decimal) {
                            return Err("balances.parts.amount must be decimal text".into());
                        }
                    }
                }
            }
        }
    }
    if let Some(limit) = value.get("limit_reached").filter(|value| !value.is_null()) {
        required_text(limit, "type")?;
        optional_time(limit, "resets_at")?;
    }
    if let Some(resets) = value
        .get("available_resets")
        .filter(|value| !value.is_null())
    {
        if resets
            .get("available_count")
            .and_then(Json::as_i64)
            .is_none_or(|count| count < 0)
        {
            return Err("available_resets.available_count must be nonnegative".into());
        }
        if let Some(credits) = list(resets, "credits", RESETS)? {
            for credit in credits {
                for member in ["id", "reset_type", "status"] {
                    required_text(credit, member)?;
                }
                optional_text(credit, "title")?;
                optional_text(credit, "description")?;
                optional_time(credit, "granted_at")?;
                optional_time(credit, "expires_at")?;
            }
        }
    }
    if let Some(credential) = value.get("credential").filter(|value| !value.is_null()) {
        if !matches!(
            credential.get("yields").and_then(Json::as_str),
            Some("login" | "token" | "key" | "none")
        ) {
            return Err("credential.yields must name a credential form".into());
        }
        optional_time(credential, "expires_at")?;
        optional_time(credential, "issued_at")?;
        optional_text(credential, "method")?;
    }
    if let Some(extra) = value.get("extra_usage").filter(|value| !value.is_null()) {
        if extra.get("enabled").and_then(Json::as_bool).is_none() {
            return Err("extra_usage.enabled must be boolean".into());
        }
        optional_bool(extra, "limit_reached")?;
        optional_text(extra, "unit")?;
        optional_text(extra, "disabled_reason")?;
        optional_count(extra, "decimal_places")?;
        for member in ["limit", "used", "used_percent"] {
            if extra.get(member).is_some_and(|value| {
                !value.is_null() && value.as_f64().is_none_or(|amount| !amount.is_finite())
            }) {
                return Err(format!("extra_usage.{member} must be numeric or null"));
            }
        }
    }
    Ok(())
}

fn optional_time(value: &Json, member: &str) -> Result<(), String> {
    if value
        .get(member)
        .is_some_and(|value| !value.is_null() && !time(value))
    {
        Err(format!("{member} must be a UTC time or null"))
    } else {
        Ok(())
    }
}
fn optional_bool(value: &Json, member: &str) -> Result<(), String> {
    if value
        .get(member)
        .is_some_and(|value| !value.is_null() && !value.is_boolean())
    {
        Err(format!("{member} must be boolean or null"))
    } else {
        Ok(())
    }
}
fn optional_count(value: &Json, member: &str) -> Result<(), String> {
    if value
        .get(member)
        .is_some_and(|value| !value.is_null() && value.as_i64().is_none_or(|count| count < 0))
    {
        Err(format!("{member} must be nonnegative or null"))
    } else {
        Ok(())
    }
}
fn optional_positive(value: &Json, member: &str) -> Result<(), String> {
    if value
        .get(member)
        .is_some_and(|value| !value.is_null() && value.as_i64().is_none_or(|count| count <= 0))
    {
        Err(format!("{member} must be positive or null"))
    } else {
        Ok(())
    }
}
fn optional_text(value: &Json, member: &str) -> Result<(), String> {
    if value.get(member).is_some_and(|value| {
        !value.is_null()
            && value
                .as_str()
                .is_none_or(|text| text.chars().count() > TEXT_CHARS)
    }) {
        Err(format!("{member} must be bounded text or null"))
    } else {
        Ok(())
    }
}

fn models_shape(value: &Json) -> Result<(), String> {
    match value.get("models") {
        Some(Json::Null) => Ok(()),
        Some(Json::Array(rows)) => {
            if rows.len() > orch_provider_contract::limits::MODELS_MAX {
                return Err("models exceeds its item bound".into());
            }
            for row in rows {
                required_text(row, "model_name")?;
                optional_text(row, "display_name")?;
                optional_bool(row, "listed")?;
                if let Some(parameters) = row.get("parameters") {
                    let _: orch_agent_contract::parameters::ModelParameters =
                        serde_json::from_value(parameters.clone())
                            .map_err(|_| "a model suggestion has malformed parameters")?;
                }
            }
            Ok(())
        }
        _ => Err("models must be explicit null or a list".into()),
    }
}

fn present_shape(value: &Json, now: Time) -> Result<(), String> {
    let headers = value
        .get("headers")
        .and_then(Json::as_object)
        .ok_or("headers must be an object")?;
    if headers.is_empty() {
        return Err("a presenter returned no headers".into());
    }
    for (name, value) in headers {
        let value = value.as_str().ok_or("a header value must be text")?;
        if http::header::HeaderName::from_bytes(name.as_bytes()).is_err()
            || http::header::HeaderValue::from_str(value).is_err()
        {
            return Err("a presented header is not valid HTTP".into());
        }
    }
    if let Some(expiry) = value.get("expires_at").filter(|value| !value.is_null())
        && (!time(expiry)
            || expiry
                .as_str()
                .and_then(|text| text.parse::<Time>().ok())
                .is_none_or(|at| at <= now))
    {
        return Err("the presented credential is already expired".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
