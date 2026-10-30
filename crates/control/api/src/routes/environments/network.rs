//! An environment's or a runtime's network as people see it: what its host's node reported, the restriction retained on it, and what its template asks for where nothing was reported yet.
//!
//! The host's node is the only one that says what is in force for a unit; until it has said so for a restricted
//! one, the network is shown as asked and not ready, never as ready. A restriction retained on an old open unit (a
//! change to restricted that did not complete) is shown beside the mode the unit has, and the unit stays stopped.
//! A directory on a person's machine has the machine's network.

use std::collections::HashMap;

use orch_api_contract::environment::{EgressMode, NetworkReadiness, NetworkState, UnitNetworkMode};
use orch_authz::Principal;
use orch_db::Conn;
use orch_db::queries::network::{self as rows, RuntimeNetworkRow};
use orch_types::{Id, Json};

use crate::middleware::requests::ApiResult;

/// What a template's `network` member asks for: the network it stands on and its egress.
fn asked(template_network: Option<&Json>) -> (UnitNetworkMode, EgressMode) {
    let read = |member: &str| {
        template_network
            .and_then(|network| network.get(member))
            .and_then(Json::as_str)
    };
    if read("mode") == Some("none") {
        return (UnitNetworkMode::None, EgressMode::Restricted);
    }
    let egress = if read("egress") == Some("restricted") {
        EgressMode::Restricted
    } else {
        EgressMode::Open
    };
    (UnitNetworkMode::Bridge, egress)
}

/// The mode a runtime's row retains, when it retains one.
fn retained(row: &RuntimeNetworkRow) -> Option<EgressMode> {
    match row.network_required.as_deref() {
        Some("restricted") => Some(EgressMode::Restricted),
        Some("open") => Some(EgressMode::Open),
        _ => None,
    }
}

/// The network of a runtime from what is stored of it; `connected` for a directory on a person's machine, which
/// has no runtime and the machine's network.
#[must_use]
pub(crate) fn state_of(row: Option<&RuntimeNetworkRow>, connected: bool) -> NetworkState {
    if connected {
        return NetworkState::open(UnitNetworkMode::Connected);
    }
    let Some(row) = row else {
        return NetworkState::open(UnitNetworkMode::Bridge);
    };
    let (network_mode, asked) = asked(row.template_network.as_ref());
    let required = retained(row).unwrap_or(asked);
    let reported = row
        .network_state
        .as_ref()
        .and_then(|state| serde_json::from_value::<NetworkState>(state.clone()).ok());
    let mut state = match reported {
        Some(reported) => NetworkState {
            network_mode,
            ..reported
        },
        None if asked == EgressMode::Open => NetworkState::open(network_mode),
        None => NetworkState {
            mode: EgressMode::Restricted,
            state: NetworkReadiness::Unavailable,
            reason: Some("its host has not said yet that its restriction is in force".to_owned()),
            ..NetworkState::open(network_mode)
        },
    };
    state.required_mode = required;
    if required == EgressMode::Restricted && state.mode == EgressMode::Open {
        state.state = NetworkReadiness::Unavailable;
        state.reason = Some(
            "kept stopped: a restriction was asked for and is not verified yet; retry it, or choose open in a new planned change"
                .to_owned(),
        );
    }
    state
}

/// The networks of the environments `ids`, by environment: from the runtime each stands on now, and the
/// machine's for those that are directories on a person's machine.
pub(crate) async fn of_environments(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    _connected: &[Id],
) -> ApiResult<HashMap<Id, NetworkState>> {
    let found = rows::of_environments(conn, ids).await?;
    let machines: HashMap<_, _> = orch_db::queries::environment_facts::of_many(conn, ids)
        .await?
        .into_iter()
        .collect();
    let mut states = HashMap::new();
    for id in ids {
        let row = found.iter().find(|row| row.environment_id == Some(*id));
        let state = if machines.get(id).is_some_and(|m| m.attaches()) {
            state_of(None, true)
        } else {
            projected(conn, row, Some(principal)).await?
        };
        states.insert(*id, state);
    }
    Ok(states)
}

/// The networks of the runtimes `ids`, by runtime.
pub(crate) async fn of_runtimes(
    conn: &mut Conn,
    ids: &[Id],
) -> ApiResult<HashMap<Id, NetworkState>> {
    let found = rows::of_runtimes(conn, ids).await?;
    let mut states = HashMap::new();
    for id in ids {
        let row = found.iter().find(|row| row.runtime_id == *id);
        // A list reads its rows' settings only where a restriction is in force or asked for: an open
        // network has no host list to apply, and a list of many open units costs what a few do.
        let plain = state_of(row, false);
        let state = if plain.mode == EgressMode::Open && plain.required_mode == EgressMode::Open {
            plain
        } else {
            projected(conn, row, None).await?
        };
        states.insert(*id, state);
    }
    Ok(states)
}

async fn projected(
    conn: &mut Conn,
    row: Option<&RuntimeNetworkRow>,
    principal: Option<&Principal>,
) -> ApiResult<NetworkState> {
    let mut state = state_of(row, false);
    let Some(row) = row else {
        return Ok(state);
    };
    let place = match (row.environment_id, row.provider_id) {
        (Some(id), _) => orch_settings::Place::Environment(id),
        (_, Some(id)) => orch_settings::Place::Provider(id),
        _ => return Ok(state),
    };
    let view = orch_settings::resolve(
        conn,
        place,
        orch_settings_contract::registry::keys::NETWORK_ALLOWED_HOSTS,
    )
    .await
    .map_err(crate::middleware::requests::ApiError::internal)?;
    let definition = serde_json::json!({"network": row.template_network});
    let policy = orch_engine::network::policy(conn, place, &definition)
        .await
        .map_err(crate::middleware::requests::ApiError::internal)?;
    if state.mode == EgressMode::Restricted {
        if state.revision != policy.revision {
            state.state = NetworkReadiness::Unavailable;
            state.reason = Some(
                "Applying the current host list; waiting for its host to acknowledge it".to_owned(),
            );
        }
        state.allowed_hosts = policy.allowed_hosts;
        state.revision = policy.revision;
    }
    let nodes: Vec<Id> = view
        .resolved
        .parts
        .iter()
        .filter_map(|part| part.source.node)
        .collect();
    let refs = match principal {
        Some(principal) => {
            crate::routes::nodes::view::refs(conn, principal, &nodes, orch_authz::Policy::default())
                .await?
        }
        None => HashMap::new(),
    };
    state.sources = view
        .resolved
        .parts
        .into_iter()
        .map(|part| {
            let scope = part
                .source
                .node
                .and_then(|id| refs.get(&id).filter(|node| !node.redacted).cloned());
            let hosts = part
                .value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .filter(|host| {
                    state
                        .allowed_hosts
                        .iter()
                        .any(|allowed| allowed.as_str() == *host)
                })
                .map(str::to_owned)
                .collect();
            orch_api_contract::environment::NetworkStateSource { scope, hosts }
        })
        .collect();
    Ok(state)
}

/// The actual and requested network shown before a replacement is accepted.
pub(crate) async fn planned(
    conn: &mut Conn,
    principal: &Principal,
    recipe: &orch_engine::updates::Recipe,
) -> ApiResult<NetworkState> {
    let rows = rows::of_runtimes(conn, &[recipe.old]).await?;
    let mut state = projected(conn, rows.first(), Some(principal)).await?;
    state.required_mode = match recipe.network_policy.mode {
        orch_driver_contract::network::EgressMode::Open => EgressMode::Open,
        orch_driver_contract::network::EgressMode::Restricted => EgressMode::Restricted,
    };
    state.allowed_hosts = recipe.network_policy.allowed_hosts.clone();
    if state.mode != state.required_mode || state.revision != recipe.network_policy.revision {
        state.state = NetworkReadiness::Unavailable;
        state.reason = Some("The replacement must verify this policy before work resumes; a failed restriction keeps the previous open unit stopped".to_owned());
    }
    state.revision = recipe.network_policy.revision.clone();
    Ok(state)
}

/// What a reader who reaches an environment only through a thread sees of its network: the mode and whether it
/// is in force, without the scopes its list came from.
#[must_use]
pub(crate) fn cut(state: NetworkState) -> NetworkState {
    NetworkState {
        proxy_url: None,
        allowed_hosts: Vec::new(),
        sources: Vec::new(),
        revision: String::new(),
        always: Vec::new(),
        always_revision: String::new(),
        reason: (state.state != NetworkReadiness::Ready)
            .then(|| "its host has not verified the requested network yet".to_owned()),
        last_denied: None,
        // The host's addressing is the environment's own fact, not something a thread shows.
        networks: Vec::new(),
        ..state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(network: Json, state: Option<Json>, required: Option<&str>) -> RuntimeNetworkRow {
        RuntimeNetworkRow {
            runtime_id: orch_types::new_id(),
            environment_id: None,
            provider_id: None,
            network_state: state,
            network_required: required.map(str::to_owned),
            template_network: Some(network),
        }
    }

    #[test]
    fn a_restricted_unit_is_never_shown_ready_before_its_host_said_so() {
        let state = state_of(
            Some(&row(
                json!({ "mode": "bridge", "egress": "restricted" }),
                None,
                None,
            )),
            false,
        );
        assert_eq!(state.mode, EgressMode::Restricted);
        assert_eq!(state.state, NetworkReadiness::Unavailable);
        let none = state_of(Some(&row(json!({ "mode": "none" }), None, None)), false);
        assert_eq!(none.network_mode, UnitNetworkMode::None);
        assert_eq!(none.required_mode, EgressMode::Restricted);
        let open = state_of(
            Some(&row(
                json!({ "mode": "bridge", "egress": "open" }),
                None,
                None,
            )),
            false,
        );
        assert_eq!(open.state, NetworkReadiness::Ready);
    }

    #[test]
    fn an_old_open_unit_that_a_restriction_was_asked_of_is_kept_stopped_and_says_why() {
        let reported = serde_json::to_value(NetworkState::open(UnitNetworkMode::Bridge)).unwrap();
        let state = state_of(
            Some(&row(
                json!({ "mode": "bridge", "egress": "open" }),
                Some(reported),
                Some("restricted"),
            )),
            false,
        );
        assert_eq!(state.mode, EgressMode::Open);
        assert_eq!(state.required_mode, EgressMode::Restricted);
        assert_eq!(state.state, NetworkReadiness::Unavailable);
        assert!(state.reason.unwrap().contains("kept stopped"));
    }

    #[test]
    fn a_directory_on_a_persons_machine_has_the_machines_network() {
        assert_eq!(
            state_of(None, true).network_mode,
            UnitNetworkMode::Connected
        );
    }

    #[test]
    fn a_thread_reader_sees_the_network_mode_and_state_without_its_private_addresses() {
        let mut state = NetworkState::open(UnitNetworkMode::Bridge);
        state.mode = EgressMode::Restricted;
        state.required_mode = EgressMode::Restricted;
        state.proxy_url = Some("http://10.199.0.2:3128".to_owned());
        state.allowed_hosts = vec!["private.example".to_owned()];
        state.revision = "private-policy".to_owned();
        state.always = vec![orch_api_contract::environment::NetworkTransport {
            authority: "private-gateway.example".to_owned(),
            protocol: "wss".to_owned(),
            port: 9443,
            purpose: orch_api_contract::environment::NetworkTransportPurpose::Gateway,
        }];
        state.networks = vec![orch_api_contract::environment::NetworkStateAddress {
            name: "orch-environment-0199abcd1234-net".to_owned(),
            role: orch_api_contract::environment::NetworkStateAddressRole::Unit,
            subnet: "10.199.4.16/28".to_owned(),
            gateway: None,
            address: Some("10.199.4.19".to_owned()),
        }];
        let shown = cut(state);
        assert_eq!(shown.mode, EgressMode::Restricted);
        assert_eq!(shown.state, NetworkReadiness::Ready);
        assert!(
            shown.proxy_url.is_none() && shown.allowed_hosts.is_empty() && shown.always.is_empty()
        );
        assert!(shown.revision.is_empty() && shown.always_revision.is_empty());
        assert!(
            shown.networks.is_empty(),
            "a thread reader is not shown the host's subnets and addresses"
        );
    }
}
