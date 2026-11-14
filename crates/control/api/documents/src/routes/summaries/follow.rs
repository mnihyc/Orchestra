//! Project existing native queue/recovery timing; a free read never schedules or retries paid work.

use super::*;

use chrono::DateTime;

use chrono::Utc;
pub(super) struct Follow {
    pub scheduled: bool,
    pub updating: bool,
    pub wait: Option<SummaryWait>,
    pub poll_after_ms: Option<u32>,
}

pub(super) async fn load(
    conn: &mut Conn,
    place: Id,
    latest: Option<&rows::Request>,
    now: DateTime<Utc>,
    seconds: u32,
) -> ApiResult<Follow> {
    let Some(request) = latest else {
        return Ok(project(None, None, now, seconds));
    };
    let recovery = if request.state == "failed" {
        orch_engine::calls::read_root(
            conn,
            place,
            orch_provider_contract::calls::Purpose::Summary,
            request.call_id,
        )
        .await
        .map_err(ApiError::engine)?
    } else {
        None
    };
    Ok(project(
        Some((
            request.state.as_str(),
            request.origin.as_str(),
            request.due_at,
        )),
        recovery
            .as_ref()
            .map(|recovery| (recovery.state.as_str(), recovery.due_at)),
        now,
        seconds,
    ))
}

fn project(
    request: Option<(&str, &str, DateTime<Utc>)>,
    recovery: Option<(&str, Option<DateTime<Utc>>)>,
    now: DateTime<Utc>,
    seconds: u32,
) -> Follow {
    let next = match (request, recovery) {
        (Some(("queued", origin, due)), _) if due > now => Some((
            if origin == "automatic" {
                SummaryWaitReason::Interval
            } else {
                SummaryWaitReason::Recovery
            },
            Some(due),
        )),
        (Some(("queued", _, _)), _) => Some((SummaryWaitReason::Queue, None)),
        (Some(("running", _, _)), _) => None,
        (_, Some(("scheduled" | "running", due))) => {
            Some((SummaryWaitReason::Recovery, due.filter(|due| *due > now)))
        }
        _ => None,
    };
    let updating = request.is_some_and(|(state, _, _)| matches!(state, "queued" | "running"))
        || recovery.is_some_and(|(state, _)| matches!(state, "scheduled" | "running"));
    let until = next.and_then(|(_, until)| until);
    let scheduled = until.is_some();
    let poll_after_ms = if let Some(until) = until {
        Some(u32::try_from((until - now).num_milliseconds().max(1)).unwrap_or(u32::MAX))
    } else if updating {
        Some(seconds.saturating_mul(1000).max(1))
    } else {
        None
    };
    Follow {
        scheduled,
        updating,
        wait: next.map(|(reason, until)| SummaryWait {
            reason,
            until: until.map(Into::into),
        }),
        poll_after_ms,
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn future_queue_waits_for_its_actual_due_time_and_eligible_work_uses_the_configured_pace() {
        let now = Utc::now();
        let future = now + chrono::Duration::minutes(15);
        let scheduled = project(Some(("queued", "automatic", future)), None, now, 5);
        assert!(scheduled.scheduled);
        assert_eq!(scheduled.poll_after_ms, Some(900_000));
        assert_eq!(
            scheduled.wait.map(|wait| wait.reason),
            Some(SummaryWaitReason::Interval)
        );
        let running = project(Some(("running", "person", now)), None, now, 9);
        assert!(!running.scheduled);
        assert_eq!(running.poll_after_ms, Some(9000));
        let recovery = project(
            Some(("failed", "person", now)),
            Some(("scheduled", Some(future))),
            now,
            5,
        );
        assert_eq!(
            recovery.wait.map(|wait| wait.reason),
            Some(SummaryWaitReason::Recovery)
        );
        assert_eq!(recovery.poll_after_ms, Some(900_000));
        assert_eq!(
            project(Some(("done", "person", now)), None, now, 5).poll_after_ms,
            None
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_future_queue_waits_for_its_actual_due_time_and_eligible_work_uses_the_configured_pace()
     {
        self::future_queue_waits_for_its_actual_due_time_and_eligible_work_uses_the_configured_pace(
        )
    }
}
