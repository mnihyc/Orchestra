//! Native binding choices share protected home/origin projection and arrangement verdicts.

use super::*;

use orch_api_contract::resource_groups::ResourceGrouping;

use orch_api_contract::resource_groups::ResourceMachineAvailability;
pub(super) async fn metadata(
    app: &crate::state::AppState,
    conn: &mut Conn,
    principal: &Principal,
    subjects: &[(String, Id)],
    projection: Option<&ChoiceProjection>,
) -> ApiResult<HashMap<(String, Id), ResourceGrouping>> {
    crate::routes::resources::grouping::Prepared::select(conn, subjects)
        .await?
        .render(
            app,
            conn,
            principal,
            projection.map(|p| &p.names),
            projection.map(|p| &p.machines),
        )
        .await
}

pub(super) fn arrangements(
    ground: &Ground<'_>,
    projection: Option<&ChoiceProjection>,
    choices: &[Arrangement],
) -> Vec<ResourceMachineAvailability> {
    let Some(machine_id) = ground
        .environment
        .and_then(|row| row.machine_id)
        .filter(|id| projection.is_none_or(|p| p.machines.contains(id)))
    else {
        return Vec::new();
    };
    if choices.is_empty() {
        return Vec::new();
    }
    let available = choices.iter().any(|choice| choice.available);
    vec![ResourceMachineAvailability {
        machine_id,
        available,
        reason: if available {
            None
        } else {
            choices.iter().find_map(|choice| choice.reason.clone())
        },
    }]
}
