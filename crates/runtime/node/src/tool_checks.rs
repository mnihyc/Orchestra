//! What a build in the tool cache offers: `node.tool.check`.
//!
//! The system checks each build of an agent program once, when it is first kept and before any unit is given it:
//! this node runs the build so that it prints its interface, and answers which of the names the system asked about
//! it lacks. The probe is the connector's ([`orch_connector_core::interface_probe`]), the one a connected machine's
//! own check of its installed program uses: the build runs in a scratch directory of the cache that is its home and
//! its working directory, with nothing of the node's environment but `PATH`, for at most the time the system gives,
//! in a network of its own where the host allows one. It runs with the node's user and without a unit's walls: the
//! vendor's checksum, verified when the build entered the cache, is what admits it, as it admits it to units.
//!
//! Native behavior is checked separately through a driver's disposable networkless unit. That stage has no
//! host-process fallback and uses the shared native cases carried by the node's runner.

use std::time::Duration;

use orch_connector_core::interface_probe::{Probe, ProbeError};
use orch_link_contract::commands::{NodeToolCheck, NodeToolCheckResult};
use tokio_util::sync::CancellationToken;

use crate::tools::{ToolCache, ToolError};

/// Runs the second stage only through a driver that provides its networkless unit.
///
/// # Errors
/// The build is absent, or no loaded driver can run an isolated check.
pub async fn conformance(
    cache: &ToolCache,
    drivers: &crate::drivers::DriverHost,
    request: &orch_link_contract::commands::NodeToolConformance,
    cancel: &CancellationToken,
) -> Result<
    orch_link_contract::commands::NodeToolConformanceResult,
    orch_link_contract::commands::CommandError,
> {
    use orch_driver_contract::driver::DriverError;
    use orch_link_contract::commands::{CommandError, ErrorCode};
    if request.case_timeout_secs == 0 || request.timeout_secs == 0 {
        return Err(CommandError::new(
            ErrorCode::Unsupported,
            "native check deadlines must be positive",
        ));
    }
    if !cache.has(&request.name, &request.version) {
        return Err(CommandError::new(
            ErrorCode::NotFound,
            "the build is not in this node's tool cache",
        ));
    }
    let programs: Vec<_> = request.programs.iter().map(String::as_str).collect();
    let program = orch_connector_core::builds::find_program(
        &cache.dir().join(&request.name),
        &request.version,
        &programs,
    )
    .ok_or_else(|| {
        CommandError::new(
            ErrorCode::NotFound,
            "the cached build has no harness executable",
        )
    })?;
    let mut unavailable = Vec::new();
    for (name, driver) in drivers.all() {
        match driver
            .extras()
            .harness_conformance(request, &program, cancel)
            .await
        {
            Err(DriverError::Unsupported(why)) => unavailable.push(format!("{name}: {why}")),
            Err(error) => return Err(crate::drivers::failure(&error)),
            Ok(result) => return Ok(result),
        }
    }
    Err(CommandError::new(
        ErrorCode::Unsupported,
        format!(
            "no driver can run an isolated native check: {}",
            unavailable.join("; ")
        ),
    ))
}

/// Runs the build and says which of the names it lacks.
///
/// # Errors
/// The build is not in the cache, or has no such program; the program could not be run, ended badly, ran longer
/// than allowed or printed more than is read; the check was cancelled.
pub async fn check(
    cache: &ToolCache,
    request: &NodeToolCheck,
    cancel: &CancellationToken,
) -> Result<NodeToolCheckResult, ToolError> {
    if !cache.has(&request.name, &request.version) {
        return Err(ToolError::Invalid(format!(
            "{} {} is not in the tool cache of this host: node.tool.ensure comes first",
            request.name, request.version
        )));
    }
    let programs: Vec<&str> = request.programs.iter().map(String::as_str).collect();
    let program = orch_connector_core::builds::find_program(
        &cache.dir().join(&request.name),
        &request.version,
        &programs,
    )
    .ok_or_else(|| {
        ToolError::Shape(format!(
            "{} {} has none of the programs {}",
            request.name,
            request.version,
            request.programs.join(", ")
        ))
    })?;
    let probe = Probe {
        args: &request.args,
        printed: request.printed,
        names: &request.names,
        timeout: Duration::from_secs(u64::from(request.timeout_secs.max(1))),
    };
    let missing =
        orch_connector_core::interface_probe::missing(&program, &probe, cache.dir(), cancel)
            .await?;
    Ok(NodeToolCheckResult { missing })
}

impl From<ProbeError> for ToolError {
    fn from(error: ProbeError) -> Self {
        match error {
            ProbeError::Io { what, source } => Self::Io { what, source },
            ProbeError::Shape(why) => Self::Shape(why),
            ProbeError::Cancelled => Self::Cancelled,
        }
    }
}
