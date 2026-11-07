/** A late refusal cannot teach another turn that it lacks steering. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Threads from '@/api/threads';
import { ApiError } from '@/api/client';
import { composerWriter, steerRefusedFor, submitInput, useComposerStore } from '@/features/composer/Draft';
import type { Thread } from '@/generated/api-types';
import { threadWriter } from '@/realtime/stores/thread';
import { defaultThreadRead } from '@/realtime/threadScope';
import { ACME } from '../doubles/fixtures';
import { turn } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { deferred } from '../component/deferred';

const send = vi.hoisted(() => vi.fn<typeof Threads.sendInput>());
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof Threads>(), sendInput: send }));
let thread: Thread;
const before = useComposerStore.getState();
function start(id: string, ordinal: number) {
  threadWriter.reset(thread.id, { thread: { ...thread, state: 'running', current_turn_id: id }, turns: [turn(id, { ordinal, state: 'running', inputId: 'input-'+ordinal })] }, false);
}
const unsupported = () => new ApiError({ type: 'orch:error:capability.unsupported', code: 'capability.unsupported', status: 409, title: 'Cannot steer', detail: 'This turn takes no steering', request_id: 'steer-attempt' }, null);
beforeEach(() => {
  send.mockReset(); useComposerStore.setState({ cannotSteer: {}, text: {}, parts: {}, sending: {}, refused: {} });
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The steering fixture needs its native thread');
  thread = world.threads.thread(person, row); start('turn-one', 1);
});
afterEach(() => { threadWriter.evict(thread.id); useComposerStore.setState(before); });

it('learns the actual refused turn, without applying that fact to its next turn or a different thread', async () => {
  send.mockRejectedValue(unsupported());
  submitInput(defaultThreadRead(thread.id), 'A refused steer', 'steer');
  await vi.waitFor(() => expect(useComposerStore.getState().cannotSteer[thread.id]).toBe('turn-one'));
  start('turn-two', 2);
  expect(steerRefusedFor(useComposerStore.getState(), thread.id, 'turn-two')).toBe(false);
  expect(steerRefusedFor(useComposerStore.getState(), 'another-thread', 'turn-one')).toBe(false);
  expect(useComposerStore.getState().text[thread.id]).toBe('A refused steer');
});

it('does not replace a current refusal with an older turn’s late answer', async () => {
  const waiting = deferred<Awaited<ReturnType<typeof Threads.sendInput>>>(); send.mockReturnValueOnce(waiting.promise);
  submitInput(defaultThreadRead(thread.id), 'Old turn steer', 'steer');
  start('turn-two', 2); composerWriter.noteCannotSteer(thread.id, 'turn-two');
  waiting.reject(unsupported());
  await vi.waitFor(() => expect(useComposerStore.getState().text[thread.id]).toBe('Old turn steer'));
  expect(useComposerStore.getState().cannotSteer[thread.id]).toBe('turn-two');
});

it('does not teach a refusal to a turn whose id was not reported for the attempt', async () => {
  threadWriter.reset(thread.id, { thread: { ...thread, state: 'starting', current_turn_id: null }, turns: [] }, false);
  send.mockRejectedValue(unsupported()); submitInput(defaultThreadRead(thread.id), 'Unknown turn steer', 'steer');
  await vi.waitFor(() => expect(useComposerStore.getState().text[thread.id]).toBe('Unknown turn steer'));
  expect(useComposerStore.getState().cannotSteer).toEqual({});
});
