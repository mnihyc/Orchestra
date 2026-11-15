/** A message sent to a thread that takes it into a turn of its own is a message at once; the queue holds only what waits behind a turn that runs. */
import { describe, expect, it } from 'vitest';
import type { SendingInput } from '@/features/composer/Draft';
import { partitionWaiting } from '@/features/thread/sentMessages';
import type { Thread, ThreadInput } from '@/generated/api-types';
import { EMPTY_VIEW, type ThreadView } from '@/realtime/stores/thread';
import { me, userRef } from '../doubles/fixtures';
import { threadInput } from '../doubles/thread-rows';
import { snapshot } from './continuation-fixtures';

const ADA = userRef(me(1, 'ada', 'Ada'));

function viewOf(state: Thread['state'], inputs: ThreadInput[]): ThreadView {
  return { ...EMPTY_VIEW, thread: { ...snapshot().thread, state }, inputs: new Map(inputs.map((input) => [input.id, input])) };
}
const sending = (clientId: string, state: SendingInput['state'] = 'sending'): SendingInput => ({ clientId, content: [{ type: 'text', text: `Words of ${clientId}` }], delivery: 'queue', state, at: '2026-10-01T10:00:00.000Z' });

describe('the messages that wait', () => {
  it.each(['idle', 'starting'] as const)('draws the first message of a thread that is %s as the person’s message and keeps it out of the queue', (state) => {
    const first = threadInput('first', 'Work in a new directory', { author: ADA, position: '1' });
    const waiting = partitionWaiting(viewOf(state, [first]), []);
    expect(waiting.message).toMatchObject({ key: 'first', content: first.content });
    expect(waiting.queued).toEqual([]);
  });

  it('keeps a message that waits behind a turn that runs in the queue', () => {
    for (const state of ['running', 'waiting_approval'] as const) {
      const behind = threadInput('behind', 'Then the tests', { author: ADA, position: '1' });
      const waiting = partitionWaiting(viewOf(state, [behind]), []);
      expect(waiting.message).toBeNull();
      expect(waiting.queued).toEqual([behind]);
    }
  });

  it('draws a message that was sent and not yet answered from the first moment, and leaves a send that failed in the queue', () => {
    const waiting = partitionWaiting(viewOf('idle', []), [sending('now')]);
    expect(waiting.message).toMatchObject({ key: 'now' });
    expect(waiting.sending).toEqual([]);
    const failed = partitionWaiting(viewOf('idle', []), [sending('lost', 'not_sent')]);
    expect(failed.message).toBeNull();
    expect(failed.sending).toHaveLength(1);
  });

  it('keeps the same message when the server accepts it: its client id is the key before and after', () => {
    const accepted = threadInput('input-1', 'Same words', { author: ADA, position: '1', clientId: 'now' });
    expect(partitionWaiting(viewOf('idle', [accepted]), [sending('now')]).message?.key).toBe('now');
  });

  it('takes only the first message into the conversation; the one behind it waits in the queue', () => {
    const one = threadInput('one', 'First', { author: ADA, position: '1' });
    const two = threadInput('two', 'Second', { author: ADA, position: '2' });
    const waiting = partitionWaiting(viewOf('idle', [one, two]), []);
    expect(waiting.message?.key).toBe('one');
    expect(waiting.queued.map((input) => input.id)).toEqual(['two']);
  });

  it('keeps a held input, an announcement and one whose files hold it in the queue', () => {
    const held = { ...threadInput('held', 'Held by a stop', { author: ADA, position: '1' }), queue_held: true };
    const announced = threadInput('told', 'Told', { author: null, position: '1', source: 'announcement' });
    const files = threadInput('files', 'With files', { author: ADA, position: '1', detail: 'attachments' });
    for (const input of [held, announced, files]) {
      const waiting = partitionWaiting(viewOf('idle', [input]), []);
      expect(waiting.message).toBeNull();
      expect(waiting.queued).toEqual([input]);
    }
  });
});
