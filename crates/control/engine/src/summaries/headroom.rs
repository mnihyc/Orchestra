//! Explicit admission budgets for new automatic subscription work. Reading a plan never calls a provider.
use super::*;
use orch_provider_contract::facts::ProviderFacts;
use orch_summarizer_contract::summarizer::{BudgetReason, BudgetWait};

fn reason(
    facts: &ProviderFacts,
    offering: &str,
    groups: &[String],
    reserve: u8,
    now: Time,
    cadence: chrono::Duration,
    renewed: Option<DateTime<Utc>>,
) -> Option<BudgetReason> {
    if reserve == 0 {
        return None;
    }
    if facts.credential_verified == Some(false) {
        return Some(BudgetReason::Measurement);
    }
    let mut measured = false;
    let mut incomplete = false;
    for window in &facts.windows {
        if window.model_family.as_ref().is_some_and(|family| {
            !family.eq_ignore_ascii_case(offering)
                && !groups
                    .iter()
                    .any(|group| group.eq_ignore_ascii_case(family))
        }) {
            continue;
        }
        let observed = facts
            .field_times
            .get(&format!("windows.{}", window.id))
            .or_else(|| facts.field_times.get("windows"));
        if observed.is_none_or(|seen| {
            *seen > now
                || now.get() - seen.get() >= cadence
                || renewed.is_some_and(|at| seen.get() < at)
        }) || window.resets_at.is_some_and(|reset| reset <= now)
            || !window.used_percent.is_finite()
            || window.used_percent < 0.0
        {
            incomplete = true;
            continue;
        }
        measured = true;
        if window.used_percent >= f64::from(100 - reserve) {
            return Some(BudgetReason::Reserve);
        }
    }
    (!measured || incomplete).then_some(BudgetReason::Measurement)
}

pub(super) async fn wait(
    conn: &mut Conn,
    offering: &orch_provider_contract::capabilities::OfferingRef,
    now: Time,
) -> Result<Option<BudgetWait>, EngineError> {
    let reserve = setting::<u8>(
        conn,
        Place::Provider(offering.provider),
        keys::DELEGATION_BACKGROUND_HEADROOM_PERCENT,
    )
    .await?
    .ok_or_else(|| EngineError::unreadable("The background plan reserve is unavailable."))?;
    if reserve == 0 {
        return Ok(None);
    }
    let row: Option<(Json, Option<DateTime<Utc>>, Vec<String>)> = sqlx::query_as(
        "SELECT p.facts, p.credential_renewed_at, m.groups FROM providers p
         JOIN provider_models m ON m.provider_id = p.id
         WHERE p.id = $1 AND m.model_name = $2 AND p.kind = 'subscription' AND p.deleted_at IS NULL",
    )
    .bind(offering.provider)
    .bind(&offering.offering)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((facts, renewed, groups)) = row else {
        return Ok(None);
    };
    let cadence =
        crate::provider_fact_reads::observation_interval(conn, offering.provider, now.get())
            .await?;
    let facts = serde_json::from_value::<ProviderFacts>(facts).unwrap_or_default();
    Ok(reason(
        &facts,
        &offering.offering,
        &groups,
        reserve,
        now,
        cadence,
        renewed,
    )
    .map(|reason| BudgetWait {
        reason,
        next: Time::new(
            now.get()
                .checked_add_signed(cadence)
                .unwrap_or(DateTime::<Utc>::MAX_UTC),
        ),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_current_applicable_windows_count_toward_an_explicit_reserve()
    -> Result<(), Box<dyn std::error::Error>> {
        let now: Time = "2026-10-08T12:00:00Z".parse()?;
        let interval = chrono::Duration::minutes(30);
        let old = Time::new(now.get() - interval);
        let mut facts: ProviderFacts = serde_json::from_value(json!({
            "windows":[{"id":"full","label":"Other","used_percent":100,"model_family":"Other","resets_at":null,"duration_minutes":null},
                       {"id":"family","label":"Family","used_percent":85,"model_family":"Family","resets_at":null,"duration_minutes":null},
                       {"id":"account","label":"Account","used_percent":20,"resets_at":null,"duration_minutes":null}],
            "field_times":{"windows.full":now,"windows.family":now,"windows.account":now}
        }))?;
        let groups = vec!["family".to_owned()];
        let decide = |facts: &ProviderFacts, reserve| {
            reason(facts, "selected", &groups, reserve, now, interval, None)
        };
        assert_eq!(decide(&facts, 0), None);
        assert_eq!(decide(&facts, 15), Some(BudgetReason::Reserve));
        assert_eq!(decide(&facts, 10), None);
        facts.field_times.insert("windows.family".into(), old);
        assert_eq!(decide(&facts, 10), Some(BudgetReason::Measurement));
        facts.field_times.insert("windows.family".into(), now);
        facts.windows[1].resets_at = Some(now);
        assert_eq!(decide(&facts, 10), Some(BudgetReason::Measurement));
        facts.windows.clear();
        assert_eq!(decide(&facts, 10), Some(BudgetReason::Measurement));
        Ok(())
    }
}
