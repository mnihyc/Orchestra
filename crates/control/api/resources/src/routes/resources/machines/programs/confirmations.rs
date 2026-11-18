//! Current program attribution comes from the server's decision and the machine's actual selection.

use super::*;
use orch_agent_programs::confirmation::Confirmation;
use orch_api_contract::machine_programs::{
    ExecutionApproval, MachineExecutionApproval, MachineExecutionSubject,
};
use orch_db::queries::execution_confirmations::{self as receipts, Decision};
use orch_link_contract::policy::{LocalPolicy, PolicyHarness, PolicyKey};
use std::collections::{BTreeMap, HashMap, HashSet};

fn subject(kind: &str, path: &str) -> MachineExecutionSubject {
    MachineExecutionSubject::Program {
        harness_kind: kind.to_owned(),
        path: path.to_owned(),
    }
}

fn matches(decision: &Decision, selection: &PolicyHarness) -> bool {
    Some(decision.confirmation) == selection.confirmation
        && matches!(&decision.subject, MachineExecutionSubject::Program { harness_kind, path }
            if *harness_kind == selection.kind && *path == selection.path)
}

/// The report comes from the connector's OS. Only a plain program name can be resolved through PATH.
fn matches_reported_program(selected: &str, reported: &str) -> bool {
    if selected == reported {
        return !selected.is_empty();
    }
    if selected.is_empty() || selected.contains(['/', '\\', ':']) {
        return false;
    }
    let file = reported.rsplit(['/', '\\']).next().unwrap_or(reported);
    file == selected
        || (reported.contains('\\')
            && [".exe", ".cmd"]
                .into_iter()
                .any(|extension| file.strip_suffix(extension) == Some(selected)))
}

fn manager(
    people: &HashMap<Id, orch_api_contract::users::UserRef>,
    by: Id,
) -> orch_api_contract::users::UserRef {
    people
        .get(&by)
        .cloned()
        .unwrap_or_else(|| orch_api_contract::users::UserRef {
            id: by,
            kind: orch_api_contract::users::UserKind::Human,
            username: String::new(),
            address: None,
            display_name: String::new(),
            avatar: orch_api_contract::users::Avatar {
                text: String::new(),
                color: 0,
            },
        })
}

pub(super) async fn projection(
    conn: &mut Conn,
    machine: Id,
    facts: &Json,
) -> ApiResult<Vec<MachineExecutionApproval>> {
    Ok(among(conn, &[(machine, facts)])
        .await?
        .remove(&machine)
        .unwrap_or_default())
}

pub(in crate::routes::resources::machines) async fn among(
    conn: &mut Conn,
    machines: &[(Id, &Json)],
) -> ApiResult<BTreeMap<Id, Vec<MachineExecutionApproval>>> {
    let ids: Vec<_> = machines.iter().map(|(id, _)| *id).collect();
    let known = receipts::among(conn, &ids).await?;
    let people: Vec<_> = known
        .values()
        .flatten()
        .map(|decision| decision.confirmation.by)
        .collect();
    let people = crate::middleware::authorization::refusals::user_refs(conn, &people).await?;
    let mut result = BTreeMap::new();
    for (machine, facts) in machines {
        let selected: Vec<PolicyHarness> = facts
            .pointer("/policy/agent.harnesses")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let reported: Vec<orch_link_contract::session::MachineFactsHarness> = facts
            .get("harnesses")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let mut kinds = HashSet::new();
        let mut approvals = Vec::new();
        for selection in selected {
            if !kinds.insert(selection.kind.clone()) {
                continue;
            }
            let Some(decision) = known
                .get(machine)
                .into_iter()
                .flatten()
                .find(|decision| matches(decision, &selection))
            else {
                continue;
            };
            let Some(program) = reported.iter().find(|program| {
                program.kind == selection.kind
                    && matches_reported_program(&selection.path, &program.path)
            }) else {
                continue;
            };
            approvals.push(MachineExecutionApproval {
                subject: subject(&program.kind, &program.path),
                approval: ExecutionApproval {
                    by: manager(&people, decision.confirmation.by),
                    at: decision.confirmation.at,
                },
            });
        }
        result.insert(*machine, approvals);
    }
    Ok(result)
}

/// Retain the actual new manager; an unchanged trusted or local choice keeps its own origin.
pub(in crate::routes::resources::machines) async fn prepare_policy(
    conn: &mut Conn,
    machine: Id,
    before: Option<&LocalPolicy>,
    values: &mut BTreeMap<PolicyKey, Json>,
    confirmation: Confirmation,
) -> ApiResult<()> {
    let Some(raw) = values.get_mut(&PolicyKey::AgentHarnesses) else {
        return Ok(());
    };
    let mut selected: Vec<PolicyHarness> =
        serde_json::from_value(raw.clone()).map_err(ApiError::internal)?;
    let known = receipts::of_machine(conn, machine).await?;
    for selection in &mut selected {
        selection.confirmation = match before.and_then(|before| before.harness(&selection.kind)) {
            Some(previous) if previous.path == selection.path => previous
                .confirmation
                .filter(|_| known.iter().any(|decision| matches(decision, previous))),
            _ => Some(confirmation),
        };
    }
    *raw = serde_json::to_value(selected).map_err(ApiError::internal)?;
    Ok(())
}

pub(super) async fn complete_program(
    conn: &mut Conn,
    machine: Id,
    request: &MachineProgramsCommand,
    confirmation: Option<Confirmation>,
    facts: &Json,
) -> ApiResult<()> {
    let (kind, path, confirming) = match request {
        MachineProgramsCommand::ConfirmHarness { kind, path } => {
            (kind.as_str(), path.as_str(), true)
        }
        MachineProgramsCommand::RemoveHarness { kind, path } => {
            (kind.as_str(), path.as_str(), false)
        }
        _ => return Ok(()),
    };
    let policy: LocalPolicy =
        serde_json::from_value(facts.get("policy").cloned().unwrap_or(Json::Null))
            .map_err(ApiError::internal)?;
    let selected = policy.harness(kind);
    let accepted = if confirming {
        selected
            .is_some_and(|selected| selected.path == path && selected.confirmation == confirmation)
            && confirmation.is_some()
    } else {
        selected.is_none()
    };
    if !accepted {
        return Err(ApiError::invalid(
            "program",
            "the machine did not retain the original program decision; refresh and try again",
        ));
    }
    receipts::clear_program(conn, machine, kind).await?;
    if let Some(confirmation) = confirmation {
        receipts::put(
            conn,
            machine,
            &Decision {
                subject: subject(kind, path),
                confirmation,
            },
        )
        .await?;
    }
    Ok(())
}

pub(in crate::routes::resources::machines) async fn complete_policy(
    conn: &mut Conn,
    machine: Id,
    asked: &BTreeMap<String, Json>,
    applied: &[String],
    policy: &LocalPolicy,
) -> ApiResult<()> {
    let key = PolicyKey::AgentHarnesses.as_str();
    if !applied.iter().any(|name| name == key) {
        return Ok(());
    }
    let selected: Vec<PolicyHarness> =
        serde_json::from_value(asked.get(key).cloned().unwrap_or(Json::Null))
            .map_err(ApiError::internal)?;
    if selected != policy.agent_harnesses {
        return Err(ApiError::invalid(
            "agent.harnesses",
            "the machine did not retain the original program choices; refresh and try again",
        ));
    }
    receipts::clear_programs(conn, machine).await?;
    for selection in selected {
        if let Some(confirmation) = selection.confirmation {
            receipts::put(
                conn,
                machine,
                &Decision {
                    subject: subject(&selection.kind, &selection.path),
                    confirmation,
                },
            )
            .await?;
        }
    }
    Ok(())
}
