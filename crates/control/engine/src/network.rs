//! Resolve runtime egress policies and enforce the durable restriction at every shared start and release gate.

use orch_db::{Conn, queries::network as rows};
use orch_driver_contract::network::{EgressMode, NetworkPolicy};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Json};

use crate::{EngineError, preflight::Blocked};

/// The requested mode; a template with no network admits ordinary open networking.
#[must_use]
pub fn mode(definition: &Json) -> EgressMode {
    let network = &definition["network"];
    if network["mode"] == "none" || network["egress"] == "restricted" {
        EgressMode::Restricted
    } else {
        EgressMode::Open
    }
}

/// Freeze the exact other names at the resource's home and its own narrowing; all-unset allows only Orch.
pub async fn policy(
    conn: &mut Conn,
    place: Place,
    definition: &Json,
) -> Result<NetworkPolicy, EngineError> {
    let mut allowed_hosts = if definition["network"]["mode"] == "none" {
        Vec::new()
    } else {
        get::<Vec<String>>(conn, place, keys::NETWORK_ALLOWED_HOSTS)
            .await?
            .unwrap_or_default()
    };
    allowed_hosts.sort_unstable();
    allowed_hosts.dedup();
    let mode = mode(definition);
    let revision = Hash::of(&serde_json::to_vec(&(mode, &allowed_hosts))?).to_string();
    Ok(NetworkPolicy {
        mode,
        allowed_hosts,
        revision,
        ..NetworkPolicy::default()
    })
}

/// A retained restriction blocks the current old open unit even after its replacement job ended.
pub async fn blocked(conn: &mut Conn, runtime: Id) -> Result<Option<Blocked>, EngineError> {
    let Some(row) = rows::of_runtimes(conn, &[runtime])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    if row.network_required.as_deref() != Some("restricted") {
        return Ok(None);
    }
    let ready = row.network_state.as_ref().is_some_and(|state| {
        state["mode"] == "restricted"
            && state["required_mode"] == "restricted"
            && state["state"] == "ready"
    });
    Ok((!ready).then(|| Blocked {
        reason: "network_restricted_unavailable",
        detail: Some("Kept stopped: the requested network restriction is not verified. Retry the change or choose open in a new plan.".to_owned()),
    }))
}

/// The common thread gate, used by lock release and every subsequent preflight without consuming queued input.
pub async fn blocked_thread(conn: &mut Conn, thread: Id) -> Result<Option<Blocked>, EngineError> {
    for runtime in rows::for_thread(conn, thread).await? {
        if let Some(blocked) = blocked(conn, runtime).await? {
            return Ok(Some(blocked));
        }
    }
    Ok(None)
}

/// Whether this runtime's own host has acknowledged this frozen policy before successor commit.
pub async fn verified(
    conn: &mut Conn,
    runtime: Id,
    policy: &NetworkPolicy,
) -> Result<bool, EngineError> {
    if !policy.restricted() {
        return Ok(true);
    }
    Ok(rows::of_runtimes(conn, &[runtime])
        .await?
        .into_iter()
        .next()
        .and_then(|row| row.network_state)
        .is_some_and(|state| {
            state["mode"] == "restricted"
                && state["state"] == "ready"
                && state["revision"].as_str() == Some(policy.revision.as_str())
                && state["always_revision"]
                    .as_str()
                    .is_some_and(|revision| !revision.is_empty())
        }))
}

/// The authenticated command to the registered node; trusted endpoints are derived there.
#[must_use]
pub fn apply(
    runtime: Id,
    policy: &NetworkPolicy,
) -> orch_link_contract::commands::NodeNetworkApply {
    orch_link_contract::commands::NodeNetworkApply {
        runtime_id: runtime,
        required_mode: match policy.mode {
            EgressMode::Open => orch_api_contract::environment::EgressMode::Open,
            EgressMode::Restricted => orch_api_contract::environment::EgressMode::Restricted,
        },
        allowed_hosts: policy.allowed_hosts.clone(),
        always: Vec::new(),
        always_revision: String::new(),
        revision: policy.revision.clone(),
    }
}

impl crate::Engine {
    /// Reconcile each live restricted unit after settings changed; a command id deduplicates one policy.
    pub(crate) async fn reconcile_networks(&self) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        for host in rows::active_hosts(&mut conn).await? {
            self.reconcile_host_networks(&mut conn, host, None).await?;
        }
        Ok(())
    }

    /// A full node report reissues current policies, with its durable sequence distinguishing reconnects.
    pub(crate) async fn reconcile_host_networks(
        &self,
        conn: &mut Conn,
        host: Id,
        sequence: Option<i64>,
    ) -> Result<(), EngineError> {
        use orch_db::queries::{environments::hosting as envs, resources::hosting as hosts};
        let Some(host_row) = hosts::host(conn, host).await? else {
            return Ok(());
        };
        for id in rows::on_host(conn, host).await? {
            let Some(runtime) = envs::runtime(conn, id).await? else {
                continue;
            };
            // A replacement freezes its policy in its job, which exclusively owns its candidate.
            if runtime.successor_of.is_some() {
                continue;
            }
            let Some(template) = hosts::template(conn, runtime.template_id).await? else {
                continue;
            };
            let place = match (runtime.environment_id, runtime.provider_id) {
                (Some(id), _) => Place::Environment(id),
                (_, Some(id)) => Place::Provider(id),
                _ => continue,
            };
            let mut definition = orch_db::queries::jobs::changes::definition(conn, id)
                .await?
                .and_then(|spec| spec.get("spec").cloned())
                .unwrap_or(template.spec);
            if rows::required(conn, id).await?.as_deref() == Some("restricted")
                && definition["network"]["mode"] != "none"
            {
                definition["network"] = serde_json::json!({"mode":"bridge", "egress":"restricted"});
            }
            let policy = policy(conn, place, &definition).await?;
            if !policy.restricted() {
                continue;
            }
            if sequence.is_none() && verified(conn, id, &policy).await? {
                continue;
            }
            let epoch = sequence.unwrap_or_default().to_le_bytes();
            let command = crate::commands::Outgoing::new(
                crate::ids::derived(
                    "node.network.apply",
                    &[id.as_bytes(), policy.revision.as_bytes(), &epoch],
                ),
                host_row.machine_id,
                orch_link_contract::commands::SerialKey::Runtime(id),
                &apply(id, &policy),
            )?;
            self.inner.commands.issue(command).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_orch_and_restricted_never_fall_back_to_open() {
        assert_eq!(
            mode(&json!({"network":{"mode":"none"}})),
            EgressMode::Restricted
        );
        assert_eq!(
            mode(&json!({"network":{"mode":"bridge","egress":"restricted"}})),
            EgressMode::Restricted
        );
        assert_eq!(
            mode(&json!({"network":{"mode":"bridge"}})),
            EgressMode::Open
        );
    }
}
