//! A later paid attempt cannot borrow an earlier paragraph's source authority.
use super::*;
use orch_api_contract::summary_facts::{
    SummaryAttemptFacts, SummaryAttemptOutcome, SummaryDiscardReason,
};

pub(super) async fn read(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    plan: &Prepared,
    wanted: Option<SummaryChatRange>,
    request: Option<&rows::Request>,
    inputs_slot: usize,
) -> ApiResult<(Option<SummaryAttemptFacts>, Vec<KeyResolvedTarget>)> {
    let Some(request) = request.filter(|row| matches!(row.state.as_str(), "failed" | "discarded"))
    else {
        return Ok((None, Vec::new()));
    };
    let (Some(recorded), Some(prepared)) = (&request.attempt_facts, &request.prepared) else {
        return Ok((None, Vec::new()));
    };
    let Some(refs) = prepared.get("refs") else {
        return Ok((None, Vec::new()));
    };
    let refs: Vec<SummaryTarget> =
        serde_json::from_value(refs.clone()).map_err(ApiError::internal)?;
    if refs.is_empty()
        || !cache_read::same_range(conn, principal, target, wanted, prepared.get("source")).await?
    {
        return Ok((None, Vec::new()));
    }
    let Some(checked) =
        cache_read::guard_refs(conn, principal, target, plan, &refs, inputs_slot, false).await?
    else {
        return Ok((None, Vec::new()));
    };
    let reason = request
        .problem
        .as_ref()
        .and_then(|problem| problem.get("reason"))
        .and_then(Json::as_str);
    let outcome = if request.state == "discarded" {
        SummaryAttemptOutcome::Discarded
    } else if matches!(reason, Some("unusable_answer" | "output_cut")) {
        SummaryAttemptOutcome::Unusable
    } else {
        SummaryAttemptOutcome::Failed
    };
    let discard = if request.state == "discarded" {
        match reason {
            Some("source_changed") => Some(SummaryDiscardReason::SourceChanged),
            Some("settings_changed") => Some(SummaryDiscardReason::ProducerChanged),
            _ => None,
        }
    } else {
        None
    };
    let mut value = recorded.clone();
    value["outcome"] = serde_json::to_value(outcome).map_err(ApiError::internal)?;
    value["discard_reason"] = serde_json::to_value(discard).map_err(ApiError::internal)?;
    // A captured identifier grants no name/account read. Reproject the actual payer through the
    // ordinary people view and keep the admitted model name without its provider configuration.
    let owner = value
        .get("usage_owner_id")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value::<Id>)
        .transpose()
        .map_err(ApiError::internal)?;
    let person = if let Some(owner) = owner {
        crate::middleware::authorization::refusals::user_refs(conn, &[owner])
            .await?
            .remove(&owner)
    } else {
        None
    };
    value["usage_owner"] = serde_json::to_value(person).map_err(ApiError::internal)?;
    let mut attempt: SummaryAttemptFacts =
        serde_json::from_value(value).map_err(ApiError::internal)?;
    if let Some(producer) = &mut attempt.producer {
        producer.provider = None;
    }
    Ok((Some(attempt), checked.inputs))
}
