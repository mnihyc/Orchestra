//! The suite every executor passes: start, the protocol its harness expects, and attribution of what it starts.
//!
//! Two independently constructed executors run together under different data directories. A fixed port,
//! shared token or stopping the other instance is a named failure rather than a hidden host assumption.

use std::time::Duration;

use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use serde::{Deserialize, Serialize};

use crate::report::{Checked, Report, skip};

/// A local executor starts or stops within the ordinary plugin deadline.
const WITHIN: Duration = Duration::from_secs(60);

/// Two directories reserved for this run, with their actual host facts and tool builds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// The first executor's environment.
    pub first: EnvironmentContext,
    /// A distinct environment and data directory on the same host.
    pub second: EnvironmentContext,
}

/// Exercises two instances concurrently and checks that stopping one leaves the other's listener usable.
pub async fn run(
    first: &mut dyn Executor,
    second: &mut dyn Executor,
    fixture: &Fixture,
    report: &mut Report,
) {
    let descriptor = first.describe();
    report.checked(
        "executor.descriptor",
        "Its protocol, tool sets, sandbox and attribution are consistent",
        descriptor.check().map_err(|faults| {
            faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
                .into()
        }),
    );
    let contexts_distinct = fixture.first.environment_id != fixture.second.environment_id
        && fixture.first.data_dir != fixture.second.data_dir;
    report.checked(
        "executor.directories",
        "Two environments have separate directories and identities",
        {
            if contexts_distinct {
                Ok(())
            } else {
                Err("the fixture must supply distinct environments and data directories".into())
            }
        },
    );
    if !contexts_distinct {
        return;
    }
    let mut endpoints = None;
    report.run("executor.start_pair", "Two executors start together on different loopback ports", WITHIN, async {
        let (a, b) = tokio::join!(first.available(&fixture.first), second.available(&fixture.second));
        if !a.available || !b.available {
            return Err(format!("the supplied environments cannot offer the executor: {:?}; {:?}", a.reason, b.reason).into());
        }
        let (a, b) = tokio::join!(first.start(&fixture.first), second.start(&fixture.second));
        let a = a.map_err(|error| error.to_string())?;
        let b = b.map_err(|error| error.to_string())?;
        let a_port = a.port().map_err(|error| error.to_string())?;
        let b_port = b.port().map_err(|error| error.to_string())?;
        endpoints = Some((a.clone(), b.clone()));
        if a_port == b_port || (a.token.is_some() && a.token == b.token) {
            return Err("the two executors share a listening port or authentication token".into());
        }
        if a.version.is_empty() || b.version.is_empty()
            || a.tool_sets.iter().chain(&b.tool_sets).any(|set| !descriptor.has_tool_set(set))
            || a.sandbox_modes.iter().chain(&b.sandbox_modes).any(|mode| !descriptor.sandbox_modes.contains(mode))
        {
            return Err("the endpoint advertises an undeclared tool set, sandbox mode or empty version".into());
        }
        reachable(&a).await?;
        reachable(&b).await
    }).await;
    report
        .run(
            "executor.stop_one",
            "Stopping one executor leaves the second usable",
            WITHIN,
            async {
                first.stop().await.map_err(|error| error.to_string())?;
                let Some((_, second)) = &endpoints else {
                    return Err(skip("neither usable pair of endpoints was reported"));
                };
                reachable(second).await
            },
        )
        .await;
    report
        .run(
            "executor.stop",
            "Both executors can be stopped, including a repeated stop",
            WITHIN,
            async {
                second.stop().await.map_err(|error| error.to_string())?;
                first.stop().await.map_err(|error| error.to_string())?;
                second
                    .stop()
                    .await
                    .map_err(|error| error.to_string().into())
            },
        )
        .await;
}

async fn reachable(endpoint: &ExecutorEndpoint) -> Checked {
    endpoint.port().map_err(|error| error.to_string())?;
    let without_scheme = endpoint
        .address
        .split_once("://")
        .map_or(endpoint.address.as_str(), |(_, rest)| rest);
    let authority = without_scheme
        .split_once('/')
        .map_or(without_scheme, |(authority, _)| authority);
    tokio::net::TcpStream::connect(authority)
        .await
        .map(|_| ())
        .map_err(|error| format!("the reported loopback endpoint is unreachable: {error}").into())
}
