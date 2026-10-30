/** Stateless model-request rows for log, navigation and pause regressions. */
import { maybeHarnessName } from './harness-names';
import type { UsageEvent } from '../../src/generated/api-types';

export function requestRow(overrides: Partial<UsageEvent> = {}): UsageEvent {
  return {
    id: '00000000-0000-4000-8000-000000000101', at: '2026-10-15T04:00:00.000Z', origin: 'harness', purpose: 'turn', scope: 'request', counted: true,
    project: { id: '00000000-0000-4000-8000-000000000102', kind: 'project', name: 'Web', redacted: false },
    topic: { id: '00000000-0000-4000-8000-000000000103', kind: 'topic', name: 'Sign in', redacted: false },
    thread: { id: '00000000-0000-4000-8000-000000000104', kind: 'thread', name: 'Repair redirects', redacted: false },
    turn_id: '00000000-0000-4000-8000-000000000105', turn_ordinal: 7,
    user: { id: '00000000-0000-4000-8000-000000000106', kind: 'human', username: 'dana', display_name: 'Dana', avatar: { text: 'D', color: 2 } },
    provider: { id: '00000000-0000-4000-8000-000000000107', name: 'Team models', kind: 'api', harness_kind: null, harness: null, address: 'Team', placement: 'managed', status: 'ready' },
    agent: null, harness_kind: 'codex', environment: null, environment_kind: 'managed', arrangement: null,
    model: 'model-a', model_display: 'Model A', effort: 'high', tier: 'fast', access_program: 'research', running: false,
    tokens: { input_tokens: 10, cached_input_tokens: 0, output_tokens: 5, reasoning_tokens: 2, total_tokens: 15 },
    request_ref: 'request-1', context_tokens: null, rate: null, rate_source: null, credits: null, ...overrides,
    harness: overrides.harness === undefined ? maybeHarnessName(overrides.harness_kind === undefined ? 'codex' : overrides.harness_kind) : overrides.harness,
  };
}
