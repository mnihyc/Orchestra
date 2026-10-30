/** Readers see one failure for repeated attempts and one owner for the next action. */
import { describe, expect, it } from 'vitest';
import type { Item, Thread, Turn } from '@/generated/api-types';
import { failureGroup, hiddenFailureItem, manualFailureActions } from '@/features/timeline/failures';
import { buildRows } from '@/features/timeline/rows';
import { EMPTY_VIEW, viewFromRows } from '@/realtime/stores/thread';
import { errorItem, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';

const failure = { code: 'model_refused', reason: 'cyber', message: 'Synthetic filter refusal', retryable: true, incomplete: true, window: null, said: null, resets_at: null };
const failed = (id: string, ordinal: number, inputId = id): Turn => ({ ...turn(id, { ordinal, inputId, state: 'failed' }), error: failure });
function native(id: string, turnId: string): Item {
  const item = errorItem(id, failure.message, { turn: turnId });
  if (item.kind !== 'error') throw new Error('An error fixture is an error item');
  return { ...item, payload: { ...item.payload, code: failure.code } };
}
const view = (turns: Turn[], items: Item[] = [], blocked = false) => viewFromRows(EMPTY_VIEW, {
  thread: { id: 'thread', seq: 1, state: blocked ? 'blocked' : 'idle', lock: null, blocked: blocked ? { reason: 'error', detail: 'call_recovery', sandbox: null } : null } as Thread,
  turns, items: items.map((item, index) => ({ ...item, seq_created: index + 1, seq_updated: index + 1 })),
}, false);

describe('failure ownership', () => {
  it('uses the terminal footer once and counts native repeated attempts even without their finer refusal reason', () => {
    const ended = failed('failed', 1);
    const errors = [native('attempt-one', ended.id), native('attempt-two', ended.id)];
    const held = view([ended], errors);
    expect(errors.every((item) => hiddenFailureItem(held, item))).toBe(true);
    expect(failureGroup(held, ended)).toEqual({ owner: ended.id, attempts: 2 });
    expect(buildRows(held, { detail: 'steps', hideHistory: false, foldOpened: new Set(), meId: null })).toEqual([{ kind: 'footer', key: 'footer:failed', turnId: 'failed' }]);
  });

  it('groups the same failure across derived continuations but keeps an independently authored failure separate', () => {
    const first = failed('first', 1), second = failed('second', 2);
    const input = userMessage('automatic', 'Continue', { author: ADA, turn: second.id, inputId: second.input_id });
    if (input.kind !== 'user_message') throw new Error('A message fixture is a user message');
    input.payload = { ...input.payload, automatic: { rule_id: 'refused', kind: failure.code, attempt: 1 } };
    const held = view([first, second], [native('one', first.id), input, native('two', second.id)]);
    expect(failureGroup(held, first)).toEqual({ owner: second.id, attempts: 2 });
    expect(failureGroup(held, second)).toEqual({ owner: second.id, attempts: 2 });
    const separate = view([first, second], [native('one', first.id), native('two', second.id)]);
    expect(failureGroup(separate, first)).toEqual({ owner: first.id, attempts: 1 });
    expect(failureGroup(separate, second)).toEqual({ owner: second.id, attempts: 1 });
  });

  it('does not hide distinct failures or infer a shared attempt for unattributed errors', () => {
    const ended = failed('failed', 1);
    const other = native('different', ended.id);
    if (other.kind !== 'error') throw new Error('An error fixture is an error item');
    other.payload = { ...other.payload, code: 'provider_busy' };
    const unattributed = { ...native('orphan', ended.id), turn_id: null };
    const held = view([ended], [other, unattributed]);
    expect(hiddenFailureItem(held, other)).toBe(false);
    expect(hiddenFailureItem(held, unattributed)).toBe(false);
  });

  it('counts before-effects retries of the same accepted input as one repeated failure', () => {
    const first = failed('first', 1, 'accepted-input'), second = failed('second', 2, 'accepted-input');
    const held = view([first, second], [native('first-attempt', first.id), native('second-attempt', second.id)]);
    expect(failureGroup(held, first)).toEqual({ owner: second.id, attempts: 2 });
    expect(failureGroup(held, second)).toEqual({ owner: second.id, attempts: 2 });
  });

  it.each(['running', 'completed', 'failed', 'interrupted'] as const)('a later %s turn owns the next act', (state) => {
    const ended = failed('first', 1);
    const later = turn('later', { ordinal: 2, inputId: 'later-input', state });
    expect(manualFailureActions(view([ended, later]), ended)).toBe(false);
    expect(manualFailureActions(view([ended]), ended)).toBe(true);
    expect(manualFailureActions(view([ended], [], true), ended)).toBe(false);
  });

  it('keeps old actions out of a history window or an active turn whose row has not arrived yet', () => {
    const ended = failed('old', 1);
    const held = view([ended]);
    expect(manualFailureActions({ ...held, hasNewer: true }, ended)).toBe(false);
    expect(manualFailureActions({ ...held, thread: { ...held.thread!, current_turn_id: 'new-native-turn' } }, ended)).toBe(false);
  });
});
