//! Bounded captured field facts use the existing batched people/path projection, without policy probes per node.
use super::*;
use orch_api_contract::settings::{SettingScope, SettingSource};
use orch_api_contract::summary_facts::{
    AutomaticFieldFacts, AutomaticFieldSchedule, AutomaticFields, AutomaticScheduleState,
    SummaryAttemptFacts,
};
use orch_types::Json;

fn value<T: DeserializeOwned>(value: Option<&Json>) -> Option<T> {
    value
        .filter(|value| !value.is_null())
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

pub(super) fn people(detail: &NodeDetail) -> Vec<Id> {
    ["title", "tags"]
        .into_iter()
        .filter_map(|field| {
            value(
                detail
                    .automatic_fields
                    .as_ref()?
                    .get(field)?
                    .get("last_attempt")?
                    .get("usage_owner_id"),
            )
        })
        .collect()
}

fn source(policy: &Json, path: &[NodeRef]) -> Option<SettingSource> {
    let recorded = policy.get("source")?;
    let scope: SettingScope = value(recorded.get("scope"))?;
    let node: Option<Id> = value(recorded.get("node"));
    let kind = match scope {
        SettingScope::Org => Some(NodeKind::Org),
        SettingScope::Project => Some(NodeKind::Project),
        SettingScope::Topic => Some(NodeKind::Topic),
        SettingScope::Thread => Some(NodeKind::Thread),
        _ => None,
    };
    let node = node.zip(kind).map(|(id, kind)| {
        path.iter()
            .find(|reference| reference.id == id)
            .cloned()
            .unwrap_or(NodeRef {
                id,
                kind,
                name: None,
                redacted: true,
                thread_display: None,
            })
    });
    Some(SettingSource { scope, node })
}

pub(super) fn project(
    detail: &NodeDetail,
    path: &[NodeRef],
    people: &HashMap<Id, UserRef>,
) -> Option<AutomaticFields> {
    if detail.automatic_fields.is_none() && detail.automatic_schedule.is_none() {
        return None;
    }
    let field = |key: &str, automatic: bool| {
        let entry = detail
            .automatic_fields
            .as_ref()
            .and_then(|fields| fields.get(key));
        let raw_attempt = entry.and_then(|entry| entry.get("last_attempt"));
        let mut last_attempt: Option<SummaryAttemptFacts> = value(raw_attempt);
        if let Some(attempt) = last_attempt.as_mut() {
            let owner: Option<Id> =
                value(raw_attempt.and_then(|attempt| attempt.get("usage_owner_id")));
            attempt.usage_owner = owner.and_then(|owner| people.get(&owner).cloned());
            // The ordinary node supplier declares no private provider-account Read facet.
            if let Some(producer) = attempt.producer.as_mut() {
                producer.provider = None;
            }
        }
        let selected = entry
            .and_then(|entry| entry.get("selected"))
            .and_then(Json::as_bool);
        let schedule = detail
            .automatic_schedule
            .as_ref()
            .filter(|_| selected.is_some() || last_attempt.is_some())
            .map(|saved| {
                let due: Option<Time> = value(saved.get("due_at"));
                let next: Option<Time> = value(saved.get("next_at"));
                let claimed = saved.get("run_id").is_some_and(|id| !id.is_null());
                let active = automatic && selected == Some(true);
                let queued = active && (due.is_some() || claimed);
                let retry = saved
                    .get("failures")
                    .and_then(Json::as_i64)
                    .is_some_and(|failures| failures > 0);
                let skipped = saved
                    .get("has_reason")
                    .and_then(Json::as_bool)
                    .unwrap_or(false);
                let policy = entry
                    .and_then(|entry| entry.get("retry_policy"))
                    .filter(|policy| !policy.is_null());
                AutomaticFieldSchedule {
                    state: if queued {
                        if retry {
                            AutomaticScheduleState::Retrying
                        } else {
                            AutomaticScheduleState::Queued
                        }
                    } else if active && skipped {
                        AutomaticScheduleState::Skipped
                    } else {
                        AutomaticScheduleState::Idle
                    },
                    reason: if active { value(saved.get("reason_code")) } else { None },
                    due_at: if active { due } else { None },
                    next_at: if active { next } else { None },
                    last_attempt_at: last_attempt.as_ref().map(|attempt| attempt.at),
                    last_success_at: value(entry.and_then(|entry| entry.get("last_success_at"))),
                    retry_ceiling: policy
                        .and_then(|policy| policy.get("ceiling"))
                        .and_then(Json::as_u64),
                    source: policy.and_then(|policy| source(policy, path)),
                }
            });
        AutomaticFieldFacts {
            last_attempt,
            schedule,
        }
    };
    Some(AutomaticFields {
        title: field("title", detail.title_source == "auto"),
        tags: field("tags", detail.tags_source.as_deref() == Some("auto")),
    })
}
