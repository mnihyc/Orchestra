/** The normalized turn measurement has quantities and no model-request boundaries or outcome. */
import { timingFixture } from '../doubles/performance';
export const nativeTiming = {
  ...timingFixture({ reported_tokens: null, asked: null, first_output: null, end: null,
    first_token_ms: { state: 'unsupported' }, generation_ms: { state: 'unsupported' }, request_ms: { state: 'unsupported' },
    speed_milli: { state: 'unsupported' }, streamed_tokens: { state: 'unsupported' }, outcome: { state: 'unsupported' }, output_streamed: null,
    web_searches: { state: 'told', value: 2 } }),
  attempt: null,
  provider_request_id: null,
  declared_counters: { web_search: { state: 'told' as const, value: 2 }, page_fetch: { state: 'told' as const, value: 0 }, extra_capacity: { state: 'unread' as const } },
};
export const nativeTokens = { input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 0 };
