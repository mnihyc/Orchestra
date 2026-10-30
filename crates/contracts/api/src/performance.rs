//! One arithmetic rule for neutral model-request boundaries, hidden reasoning and streaming eligibility.

use crate::usage::{RequestSample, RequestTiming, UsageMeasurement};

/// Bursts shorter than half a second measure buffering and scheduling more than token generation.
pub const MIN_GENERATION_MS: i64 = 500;
/// A few dozen streamed tokens are needed to compare generation rates rather than single bursts.
pub const MIN_STREAMED_TOKENS: i64 = 32;
/// Public speeds retain three decimal places without floating-point wire differences.
pub const SPEED_SCALE: i64 = 1000;
/// The observation timestamps use milliseconds.
pub const MILLIS_PER_SECOND: i64 = 1000;

fn elapsed(
    from: Option<orch_types::Time>,
    to: Option<orch_types::Time>,
    running: bool,
) -> UsageMeasurement<i64> {
    match (from, to) {
        (Some(from), Some(to)) if to >= from => UsageMeasurement::Told {
            value: to.unix_millis() - from.unix_millis(),
        },
        (Some(_), None) if running => UsageMeasurement::Unread,
        _ => UsageMeasurement::Unsupported,
    }
}

/// Turns observations into the same measured members for every harness and provider kind.
#[must_use]
pub fn request_timing(request: &RequestSample) -> RequestTiming {
    let running = matches!(request.outcome, UsageMeasurement::Unread);
    let first = elapsed(request.asked, request.first_output, running);
    let generation = if running && request.asked.is_some() && request.first_output.is_none() {
        UsageMeasurement::Unread
    } else {
        elapsed(request.first_output, request.end, running)
    };
    let whole = elapsed(request.asked, request.end, running);
    let streamed = if request.reasoning_hidden {
        request
            .output_tokens
            .zip(request.reasoning_tokens)
            .map(|(output, reasoning)| (output - reasoning.max(0)).max(0))
    } else {
        request.output_tokens.map(|n| n.max(0))
    };
    let speed = match (streamed, generation.value()) {
        (Some(tokens), Some(ms))
            if request.output_streamed == Some(true)
                && *ms >= MIN_GENERATION_MS
                && tokens >= MIN_STREAMED_TOKENS =>
        {
            UsageMeasurement::Told {
                value: (tokens as f64 * (MILLIS_PER_SECOND * SPEED_SCALE) as f64 / *ms as f64)
                    .round() as i64,
            }
        }
        _ if running => UsageMeasurement::Unread,
        _ if request.output_streamed != Some(true) => UsageMeasurement::Unsupported,
        (None, _) => UsageMeasurement::Unsupported,
        (_, Some(_)) => UsageMeasurement::Empty,
        _ => UsageMeasurement::Unsupported,
    };
    RequestTiming {
        subagent: request.subagent,
        asked: request.asked,
        first_output: request.first_output,
        end: request.end,
        reported_tokens: request.reported_tokens.clone(),
        output_streamed: request.output_streamed,
        vendor_words: request.vendor_words.clone(),
        speed_milli: speed,
        streamed_tokens: streamed.map_or(UsageMeasurement::Unsupported, |value| {
            UsageMeasurement::Told { value }
        }),
        reasoning_hidden: request.reasoning_hidden,
        first_token_ms: if request.first_output.is_none()
            && request.asked.is_some()
            && !running
            && request.output_tokens == Some(0)
        {
            UsageMeasurement::Empty
        } else {
            first
        },
        generation_ms: generation,
        request_ms: whole,
        outcome: request.outcome.clone(),
        error_message: request.error_message.clone(),
    }
}

/// Nearest-rank median, retaining a measured zero and returning no value for no samples.
pub fn median(mut values: Vec<i64>) -> Option<i64> {
    const MIDDLE: usize = 2;
    values.sort_unstable();
    values.get(values.len().saturating_sub(1) / MIDDLE).copied()
}

/// Timed main-stream calls only; tool time includes sub-agent work and any unmeasured intervals.
pub fn turn_times(
    start: Option<orch_types::Time>,
    end: Option<orch_types::Time>,
    requests: &[RequestTiming],
) -> (Option<i64>, Option<i64>) {
    if requests.is_empty() || end.is_none() {
        return (None, None);
    }
    let times: Vec<_> = requests
        .iter()
        .filter(|request| !request.subagent)
        .filter_map(|request| request.request_ms.value().copied())
        .collect();
    let model = (!times.is_empty()).then(|| times.into_iter().sum::<i64>());
    let tool = start
        .zip(end)
        .zip(model)
        .map(|((start, end), model)| (end.unix_millis() - start.unix_millis() - model).max(0));
    (model, tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> RequestSample {
        RequestSample {
            output_tokens: Some(395),
            output_streamed: Some(true),
            asked: Some("2026-10-15T00:00:00Z".parse().unwrap()),
            first_output: Some("2026-10-15T00:00:05.630Z".parse().unwrap()),
            end: Some("2026-10-15T00:00:09.810Z".parse().unwrap()),
            reasoning_hidden: true,
            reasoning_tokens: Some(142),
            outcome: UsageMeasurement::Told {
                value: "done".into(),
            },
            ..RequestSample::default()
        }
    }
    #[test]
    fn hidden_reasoning_is_removed_and_unseparated_hidden_thinking_has_no_speed() {
        let mut raw = request();
        let measured = request_timing(&raw);
        assert_eq!(measured.first_token_ms.value(), Some(&5630));
        assert_eq!(measured.generation_ms.value(), Some(&4180));
        assert_eq!(measured.streamed_tokens.value(), Some(&253));
        assert_eq!(measured.speed_milli.value(), Some(&60_526));
        raw.reasoning_tokens = None;
        assert_eq!(
            request_timing(&raw).speed_milli,
            UsageMeasurement::Unsupported
        );
    }
    #[test]
    fn short_bursts_and_small_samples_are_unmeasured_while_real_zero_latency_is_retained() {
        let mut raw = request();
        raw.reasoning_hidden = false;
        raw.first_output = raw.asked;
        raw.end = Some("2026-10-15T00:00:00.499Z".parse().unwrap());
        assert_eq!(request_timing(&raw).first_token_ms.value(), Some(&0));
        assert_eq!(request_timing(&raw).speed_milli, UsageMeasurement::Empty);
        raw.end = Some("2026-10-15T00:00:00.500Z".parse().unwrap());
        raw.output_tokens = Some(31);
        assert_eq!(request_timing(&raw).speed_milli, UsageMeasurement::Empty);
        raw.output_tokens = Some(32);
        assert_eq!(request_timing(&raw).speed_milli.value(), Some(&64_000));
        raw.asked = None;
        raw.first_output = None;
        raw.end = None;
        assert_eq!(
            request_timing(&raw).speed_milli,
            UsageMeasurement::Unsupported
        );
        assert_eq!(
            request_timing(&raw).first_token_ms,
            UsageMeasurement::Unsupported
        );
    }

    #[test]
    fn turn_details_add_model_requests_and_leave_tool_waits_out_of_their_speed() {
        let raw = request();
        let timing = request_timing(&raw);
        let end: orch_types::Time = "2026-10-15T00:00:20Z".parse().unwrap();
        assert_eq!(
            turn_times(raw.asked, Some(end), std::slice::from_ref(&timing)),
            (Some(9810), Some(10190))
        );
        let mut child = timing.clone();
        child.subagent = true;
        assert_eq!(
            turn_times(
                raw.asked,
                Some(end),
                &[timing, child, RequestTiming::default()]
            ),
            (Some(9810), Some(10190))
        );
        assert_eq!(
            turn_times(raw.asked, Some(end), &[RequestTiming::default()]),
            (None, None)
        );
        assert_eq!(median(vec![0, 32, 64, 128]), Some(32));
        assert_eq!(median(Vec::new()), None);
    }
}
