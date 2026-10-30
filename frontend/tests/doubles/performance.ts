/** Literal request measurements and merged histogram answers for the fake world's Performance routes. */
import type { RequestTiming, UsagePerformance } from '../../src/generated/api-types.ts';
import { providerError } from '../../src/features/usage/outcomes.ts';

export function timingFixture(changes: Partial<RequestTiming> = {}): RequestTiming {
  return { subagent: false, asked: null, first_output: null, end: null, reported_tokens: { input_tokens: 100, cached_input_tokens: 20, cache_write_tokens: null, output_tokens: 64, reasoning_tokens: 0, total_tokens: 164 }, output_streamed: true, vendor_words: null,
    speed_milli: { state: 'told', value: 64_000 }, streamed_tokens: { state: 'told', value: 64 }, reasoning_hidden: false,
    first_token_ms: { state: 'told', value: 512 }, generation_ms: { state: 'told', value: 1000 }, request_ms: { state: 'told', value: 1512 }, outcome: { state: 'told', value: 'done' }, error_message: null, ...changes };
}

/** Same published bucket resolution as the real route, using already-measured fixture values. */
function percentile(values: number[], percent: number, speed = false): number | null {
  if (values.length === 0) return null;
  const buckets = values.map((n) => {
    const value = speed ? n / 1000 : n;
    if (value === 0) return 0;
    const step = Math.max(-80, Math.ceil(Math.log2(value) * 8));
    return step > 160 ? Number.POSITIVE_INFINITY : 2 ** ((step - 0.5) / 8);
  }).sort((a, b) => a - b);
  const value = buckets[Math.ceil(values.length * percent / 100) - 1];
  return value === undefined || !Number.isFinite(value) ? null : speed ? Math.round(value * 1000) : Math.ceil(value);
}

export function performanceOf(timings: readonly RequestTiming[]): UsagePerformance {
  const complete = timings.filter((timing) => timing.outcome.state === 'told');
  const speed = complete.flatMap((timing) => timing.speed_milli.state === 'told' ? [timing.speed_milli.value] : []);
  const first = complete.flatMap((timing) => timing.first_token_ms.state === 'told' ? [timing.first_token_ms.value] : []);
  const errors: Record<string, number> = {};
  for (const timing of complete) if (timing.outcome.state === 'told' && providerError(timing.outcome.value)) errors[timing.outcome.value] = (errors[timing.outcome.value] ?? 0) + 1;
  return { requests: complete.length, errors, hidden_requests: complete.filter((timing) => timing.reasoning_hidden).length, short_requests: complete.filter((timing) => timing.speed_milli.state === 'empty').length,
    speed_milli: percentile(speed, 50, true), speed_p90_milli: percentile(speed, 90, true), first_token_ms: percentile(first, 50), first_token_p90_ms: percentile(first, 90), speed_samples: speed.length, first_token_samples: first.length };
}
