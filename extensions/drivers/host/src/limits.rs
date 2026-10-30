//! Enforced or reported resource constraints.
//!
//! Limits are promised only after this node successfully creates a systemd scope with the needed controls.

use orch_driver_contract::runtime::Resources;
use std::time::Duration;

/// A local scope probe is bounded to five seconds, including an unavailable manager or authorization service.
const PROBE_WITHIN: Duration = Duration::from_secs(5);

/// Whether a native scope with CPU, memory and task controls works for this node now.
pub async fn scopes_available() -> bool {
    let controls = probe().await;
    controls.cpu && controls.memory && controls.pids
}

/// The same resource facts as a directory's user controls, proved per controller for this account.
pub async fn probe() -> orch_driver_contract::process_limits::ProcessControls {
    #[cfg(target_os = "linux")]
    {
        let mut available = [false; 3];
        for (index, property) in orch_driver_contract::process_limits::PROBES
            .into_iter()
            .enumerate()
        {
            let name = format!("orch-host-probe-{}.scope", orch_types::new_id().simple());
            let mut command = tokio::process::Command::new("systemd-run");
            command
                .args([
                    "--scope",
                    "--quiet",
                    "--expand-environment=no",
                    "--unit",
                    &name,
                    &format!("--property={property}"),
                    "/bin/true",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            available[index] = matches!(tokio::time::timeout(PROBE_WITHIN, command.status()).await, Ok(Ok(status)) if status.success());
        }
        orch_driver_contract::process_limits::ProcessControls {
            cpu: available[0],
            memory: available[1],
            pids: available[2],
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Default::default()
    }
}

/// Scope properties for positive CPU, memory and task limits; a disk quota is never invented.
#[must_use]
pub fn properties(resources: &Resources) -> Vec<String> {
    orch_driver_contract::process_limits::ProcessControls {
        cpu: true,
        memory: true,
        pids: true,
    }
    .properties(resources, false)
    .into_iter()
    .map(|property| format!("--property={property}"))
    .collect()
}
