//! Native aggregates join the existing immutable ledger without inventing model requests or timing.
use super::*;
use orch_api_contract::context::TurnUsageDetail;
use orch_api_contract::usage::{RequestTiming, UsageMeasurement};
use orch_provider_contract::capabilities::{catalogue, counters};

impl Engine {
    pub(super) async fn native_request_billing(
        &self,
        unit: &mut Unit<'_>,
        attribution: &Attribution,
        model: &str,
        event: Id,
        timing: &RequestTiming,
    ) -> Result<(RequestTiming, BTreeMap<String, i64>), EngineError> {
        let declarations = catalogue().map_err(EngineError::unreadable)?;
        let encoded = serde_json::to_value(timing)?;
        let mut requested = counters::from_measurements(&declarations.capabilities, &encoded);
        for (capability, quantity) in &timing.declared_counters {
            if let UsageMeasurement::Told { value } = quantity
                && *value >= 0
            {
                if requested
                    .get(capability)
                    .is_some_and(|known| known != value)
                {
                    requested.remove(capability);
                } else {
                    requested.insert(capability.clone(), *value);
                }
            }
        }
        let covered = ledger::native::covered(
            unit.conn,
            attribution.turn_id,
            attribution.provider_id,
            model,
            event,
            &requested,
        )
        .await?;
        if covered.quantities.is_empty() {
            return Ok((timing.clone(), covered.quantities));
        }
        let mut billed = encoded;
        for (capability, quantity) in &covered.priced {
            let Some(reported) = requested.get(capability) else {
                continue;
            };
            let remaining = reported - quantity;
            let value = serde_json::to_value(UsageMeasurement::Told { value: remaining })?;
            billed["declared_counters"][capability] = value.clone();
            if let Some(alias) = declarations
                .capabilities
                .get(capability)
                .and_then(|definition| definition.counter_accounting.as_ref())
                .and_then(|accounting| accounting.measurement.as_deref())
            {
                billed[alias] = value;
            }
        }
        Ok((serde_json::from_value(billed)?, covered.quantities))
    }
    pub(super) async fn count_native_capabilities(
        &self,
        unit: &mut Unit<'_>,
        turn: &threads::TurnRow,
    ) -> Result<(), EngineError> {
        if turn.lease_id.is_some() {
            return Ok(());
        }
        let detail: TurnUsageDetail =
            serde_json::from_value(turn.usage_detail.clone().unwrap_or_default())
                .unwrap_or_default();
        let mut active_models: std::collections::BTreeSet<String> = detail
            .models
            .iter()
            .filter(|model| model.tokens_reported && model.tokens.total_tokens > 0)
            .map(|model| model.model.clone())
            .collect();
        active_models.insert(
            detail
                .effective_model
                .as_ref()
                .unwrap_or(&turn.model)
                .clone(),
        );
        let mut reported: BTreeMap<(String, String), Option<i64>> = BTreeMap::new();
        for model in detail.models {
            if model.model.is_empty() {
                continue;
            }
            for (capability, count) in model.counts {
                if capability.is_empty() || count < 0 {
                    continue;
                }
                let slot = reported
                    .entry((model.model.clone(), capability))
                    .or_insert(Some(count));
                if *slot != Some(count) {
                    *slot = None;
                }
            }
        }
        if reported.is_empty() {
            return Ok(());
        }
        let forwarded = sqlx::query_scalar::<_, bool>(
            "SELECT credential_path='forwarded' FROM segments WHERE id=$1",
        )
        .bind(turn.segment_id)
        .fetch_optional(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?
        .unwrap_or(true);
        if forwarded {
            return Ok(());
        }
        let declarations = catalogue().map_err(EngineError::unreadable)?;
        let aliases: BTreeMap<String, Option<String>> = reported
            .keys()
            .map(|(_, capability)| {
                (
                    capability.clone(),
                    declarations
                        .capabilities
                        .get(capability)
                        .and_then(|definition| definition.counter_accounting.as_ref())
                        .and_then(|accounting| accounting.measurement.clone()),
                )
            })
            .collect();
        let Some(attribution) = threads::binding::attribution_of_turn(unit.conn, turn.id).await?
        else {
            return Ok(());
        };
        let recorded = {
            ledger::native::lock(unit.conn, turn.id, attribution.provider_id).await?;
            ledger::counted_capabilities(unit.conn, turn.id, attribution.provider_id, &aliases)
                .await?
        };
        let requests: Vec<String> = sqlx::query_scalar("SELECT DISTINCT model FROM usage_events WHERE turn_id=$1 AND provider_id=$2 AND origin='harness' AND scope='request'")
            .bind(turn.id).bind(attribution.provider_id).fetch_all(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        active_models.extend(requests);
        let book = RateBook::load(unit.conn, attribution.thread_id).await?;
        let at = turn.completed_at.unwrap_or_else(|| unit.now());
        for ((model, capability), reported) in reported {
            let Some(reported) = reported else {
                continue;
            };
            let prior = recorded.get(&(model.clone(), capability.clone()));
            let Some(counted) = prior.copied().unwrap_or(Some(0)) else {
                continue;
            };
            let Some(quantity) = counters::remainder(reported, counted) else {
                tracing::warn!(turn=%turn.id, %model, %capability, "native capability total is below its recorded request counts");
                continue;
            };
            // Old cumulative model entries with no activity must not manufacture model/turn membership.
            // A completely covered total already exists on real request rows; only a new known zero needs a trace.
            if quantity == 0 && (prior.is_some() || !active_models.contains(&model)) {
                continue;
            }
            let entry = resources::model(unit.conn, attribution.provider_id, &model).await?;
            let own = entry
                .and_then(|entry| serde_json::from_value::<ModelParameters>(entry.parameters).ok())
                .and_then(|parameters| parameters.rate);
            let found = book.find(unit.conn, own.as_ref(), &model).await?;
            let rate = found
                .as_ref()
                .map(|found| serde_json::to_value(&found.rate))
                .transpose()?;
            let accounting = declarations
                .capabilities
                .get(&capability)
                .and_then(|definition| definition.counter_accounting.as_ref());
            let credits = accounting
                .and_then(|accounting| accounting.price.as_deref())
                .and_then(|pointer| rate.as_ref()?.pointer(pointer)?.as_str())
                .and_then(|price| crate::rates::credits_for_counter(price, quantity));
            let mut timing = RequestTiming {
                declared_counters: BTreeMap::from([(
                    capability.clone(),
                    UsageMeasurement::Told { value: quantity },
                )]),
                ..RequestTiming::default()
            };
            // These are compatibility aliases of existing numeric units, never a new capability selector.
            match accounting.and_then(|accounting| accounting.measurement.as_deref()) {
                Some("web_searches") => {
                    timing.web_searches = UsageMeasurement::Told { value: quantity }
                }
                Some("web_fetches") => {
                    timing.web_fetches = UsageMeasurement::Told { value: quantity }
                }
                _ => {}
            }
            ledger::record_program(
                unit.conn,
                &NewEvent {
                    id: ids::derived(
                        "usage.native-counter",
                        &[
                            turn.id.as_bytes().as_slice(),
                            model.as_bytes(),
                            capability.as_bytes(),
                        ],
                    ),
                    at,
                    origin: ledger::origin::HARNESS,
                    scope: ledger::scope::TURN,
                    counted: quantity > 0,
                    attribution: &attribution,
                    model: &model,
                    tier: None,
                    tokens: Tokens::default(),
                    context_tokens: None,
                    request_ref: None,
                    rate: rate.as_ref(),
                    rate_source: found.as_ref().map(|found| found.source.word()),
                    credits: credits.as_deref(),
                },
                Some(&timing),
                None,
            )
            .await?;
        }
        Ok(())
    }
}
