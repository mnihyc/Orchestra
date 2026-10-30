/** A running native thread and its existing counted/streamed values for status-item proofs. */
import { i18n } from '@/i18n';
import type { StatusContext } from '@/features/status-items/registry';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { turn } from '../doubles/thread-rows';

export function statusContext(overrides: Partial<StatusContext> = {}): StatusContext {
  const world = new World(), person = world.people.get('ada'), node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('The native thread fixture is required');
  const thread = { ...world.threads.thread(person, node), state: 'running' as const };
  const running = turn('live-turn', { ordinal: 1, state: 'running', inputId: 'input', startedAt: '2026-10-25T00:00:00Z' });
  thread.current_turn_id = running.id;
  running.usage = { input_tokens: 100, output_tokens: 480, total_tokens: 580, cached_input_tokens: 0, cache_write_tokens: 0, reasoning_tokens: 0 };
  return { t: i18n.t, locale: 'en', timeZone: 'UTC', now: Date.parse('2026-10-25T00:00:10Z'), thread, turn: running,
    outputCounts: { ascii: 0, other: 0 }, tokenEstimate: { ascii_prose_x100: 482, ascii_structured_x100: 300, other_x100: 132, image_tokens: 1000, measured: true }, provider: undefined, environment: undefined, host: undefined, sample: null, ...overrides };
}
