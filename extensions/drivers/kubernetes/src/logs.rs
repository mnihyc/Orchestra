//! Bounded native logs fenced to a Pod and container incarnation.

use futures::AsyncReadExt;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::LogParams;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::units::UnitRef;
use std::time::Duration;

use crate::ownership::check_pod;

/// The workload container holds the managed connector and its output.
const WORKLOAD: &str = "workload";
/// Small bounded reads keep the log buffer limited while allowing a stalled connection to be noticed.
const READ_CHUNK_BYTES: usize = 8 * 1024;
/// Four MiB permits an interactive diagnostic read while bounding one response's memory.
/// The shared log interface still needs a continuation before this is exposed.
const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;

fn log_limit(max_bytes: i64) -> Result<usize, DriverError> {
    usize::try_from(max_bytes)
        .ok()
        .filter(|limit| (1..=MAX_LOG_BYTES).contains(limit))
        .ok_or_else(|| {
            DriverError::InvalidSpec("the log byte allowance is outside the supported range".into())
        })
}

fn container_identity(pod: &Pod) -> Result<(String, i32), DriverError> {
    if pod
        .spec
        .as_ref()
        .and_then(|spec| spec.restart_policy.as_deref())
        != Some("Never")
    {
        return Err(DriverError::Unavailable(
            "the workload container can restart behind this unit".into(),
        ));
    }
    let statuses = pod
        .status
        .as_ref()
        .and_then(|status| status.container_statuses.as_ref())
        .ok_or_else(|| {
            DriverError::Unavailable("the workload container has not reported its identity".into())
        })?;
    let mut matching = statuses.iter().filter(|status| status.name == WORKLOAD);
    let status = matching.next().ok_or_else(|| {
        DriverError::Unavailable("the workload container has not reported its identity".into())
    })?;
    if matching.next().is_some() {
        return Err(DriverError::Unavailable(
            "the workload container identity is ambiguous".into(),
        ));
    }
    let id = status
        .container_id
        .clone()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            DriverError::Unavailable("the workload container has not reported its identity".into())
        })?;
    if status.restart_count != 0 {
        return Err(DriverError::Unavailable(
            "the workload container restarted behind this unit".into(),
        ));
    }
    crate::process_scope::container_id(&id)?;
    Ok((id, status.restart_count))
}

async fn read_bounded<R: futures::AsyncRead + Unpin>(
    stream: &mut R,
    limit: usize,
    stall_timeout: Duration,
    deadline: tokio::time::Instant,
) -> Result<Vec<u8>, DriverError> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    while bytes.len() < limit {
        let remaining = (limit - bytes.len()).min(chunk.len());
        let count = tokio::time::timeout_at(
            deadline,
            tokio::time::timeout(stall_timeout, stream.read(&mut chunk[..remaining])),
        )
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| DriverError::Unavailable("the workload logs could not be read".into()))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    if tokio::time::Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(bytes)
}

/// An exact log source and the caller's unchanged byte and time allowances.
pub(crate) struct LogRead<'a> {
    pub namespace: &'a str,
    pub system: &'a str,
    pub unit: &'a UnitRef,
    pub max_bytes: i64,
    pub request_timeout: Duration,
    pub stall_timeout: Duration,
    pub deadline: tokio::time::Instant,
}

/// Reads at most the caller's byte allowance and refuses an object or container replaced during the read.
pub(crate) async fn read_logs(
    pods: &Api<Pod>,
    request: LogRead<'_>,
) -> Result<String, DriverError> {
    let LogRead {
        namespace,
        system,
        unit,
        max_bytes,
        request_timeout,
        stall_timeout,
        deadline,
    } = request;
    let limit = log_limit(max_bytes)?;
    if request_timeout.is_zero()
        || stall_timeout.is_zero()
        || tokio::time::Instant::now() >= deadline
    {
        return Err(DriverError::Timeout);
    }
    let before = tokio::time::timeout_at(
        deadline,
        tokio::time::timeout(request_timeout, pods.get_opt(&unit.name)),
    )
    .await
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| DriverError::Unavailable("the workload Pod could not be read".into()))?
    .ok_or_else(|| DriverError::NotFound("the workload Pod is absent".into()))?;
    check_pod(&before.metadata, namespace, system, unit)?;
    if before.metadata.deletion_timestamp.is_some() {
        return Err(DriverError::Unavailable(
            "the workload Pod is terminating".into(),
        ));
    }
    let identity = container_identity(&before)?;
    let options = LogParams {
        container: Some(WORKLOAD.into()),
        limit_bytes: Some(max_bytes),
        ..LogParams::default()
    };
    let stream = tokio::time::timeout_at(
        deadline,
        tokio::time::timeout(request_timeout, pods.log_stream(&unit.name, &options)),
    )
    .await
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| DriverError::Unavailable("the workload logs could not be read".into()))?;
    futures::pin_mut!(stream);
    let bytes = read_bounded(&mut stream, limit, stall_timeout, deadline).await?;
    let after = tokio::time::timeout_at(
        deadline,
        tokio::time::timeout(request_timeout, pods.get_opt(&unit.name)),
    )
    .await
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| DriverError::Unavailable("the workload Pod could not be rechecked".into()))?
    .ok_or_else(|| DriverError::NotFound("the workload Pod is absent".into()))?;
    check_pod(&after.metadata, namespace, system, unit)?;
    if after.metadata.deletion_timestamp.is_some() || container_identity(&after)? != identity {
        return Err(DriverError::NotFound(
            "the workload container was replaced".into(),
        ));
    }
    let mut logs = String::from_utf8_lossy(&bytes).into_owned();
    if logs.len() > limit {
        // Replacement characters can expand invalid bytes; keep valid UTF-8 within the same bound.
        let mut end = limit;
        while !logs.is_char_boundary(end) {
            end -= 1;
        }
        logs.truncate(end);
    }
    if tokio::time::Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_container_without_a_native_identity_is_not_treated_as_a_log_source() {
        let pod: Pod = serde_json::from_value(json!({"metadata":{"name":"test"},
            "spec":{"restartPolicy":"Never","containers":[{"name":"workload","image":"example"}]},
            "status":{"containerStatuses":[{"name":"workload","ready":false,"restartCount":0}]}}))
        .unwrap();
        assert!(container_identity(&pod).is_err());
        let id = format!("containerd://{}", "a".repeat(64));
        let pod: Pod = serde_json::from_value(json!({"metadata":{"name":"test"},
            "spec":{"restartPolicy":"Never","containers":[{"name":"workload","image":"example"}]},
            "status":{"containerStatuses":[{"name":"workload","ready":true,"restartCount":0,"containerID":id.clone()}]}})).unwrap();
        assert_eq!(container_identity(&pod).unwrap(), (id, 0));
        let mut short = pod.clone();
        short
            .status
            .as_mut()
            .unwrap()
            .container_statuses
            .as_mut()
            .unwrap()[0]
            .container_id = Some("containerd://old".into());
        assert!(container_identity(&short).is_err());
        let mut duplicate = pod.clone();
        let statuses = duplicate
            .status
            .as_mut()
            .unwrap()
            .container_statuses
            .as_mut()
            .unwrap();
        statuses.push(statuses[0].clone());
        assert!(container_identity(&duplicate).is_err());
        let mut restarted = pod;
        restarted
            .status
            .as_mut()
            .unwrap()
            .container_statuses
            .as_mut()
            .unwrap()[0]
            .restart_count = 1;
        assert!(container_identity(&restarted).is_err());
    }

    #[tokio::test]
    async fn log_reads_enforce_both_the_byte_limit_and_the_absolute_deadline() {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        assert!(log_limit(0).is_err());
        assert!(log_limit(i64::try_from(MAX_LOG_BYTES).unwrap() + 1).is_err());
        let mut input = futures::io::Cursor::new(b"abcdef".to_vec());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        assert_eq!(
            read_bounded(&mut input, 4, Duration::from_secs(1), deadline)
                .await
                .unwrap(),
            b"abcd"
        );

        struct Pending;
        impl futures::AsyncRead for Pending {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buf: &mut [u8],
            ) -> Poll<std::io::Result<usize>> {
                Poll::Pending
            }
        }
        let started = tokio::time::Instant::now();
        assert!(matches!(
            read_bounded(
                &mut Pending,
                1,
                Duration::from_secs(5),
                started + Duration::from_millis(30)
            )
            .await,
            Err(DriverError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
