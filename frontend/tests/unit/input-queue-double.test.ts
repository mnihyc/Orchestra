// @vitest-environment node
/** Pending-input writes in the browser's fake API keep identity, holds and atomic refusal semantics. */
import { describe, expect, it } from 'vitest';
import type { ThreadInput, ThreadInputUpdateReply } from '../../src/generated/api-types.ts';
import { ACME, userRef } from '../doubles/fixtures.ts';
import { threadInput, turn } from '../doubles/thread-rows.ts';
import { Refusal, World } from '../doubles/world.ts';

function rig() {
  const world = new World(), ada = world.people.get('ada')!, query = new URLSearchParams();
  const one = { ...threadInput('first', 'Original', { author: userRef(ada.me), position: '1', detail: 'cap_reached' }), queue_held: true };
  const two = threadInput('second', 'Second', { author: userRef(ada.me), position: '2' });
  world.threads.emit(ACME.mainThread, { inputs: [one, two] });
  const patch = (id: string, body: Record<string, unknown>) => world.threads.handle(ada, 'PATCH', `/threads/${ACME.mainThread}/inputs/${id}`, query, body)?.body as ThreadInputUpdateReply;
  const read = () => world.threads.snapshot(ACME.mainThread) as { inputs: ThreadInput[] };
  return { world, ada, patch, read };
}

describe('native pending-input double routes', () => {
  it('changes a held input’s delivery and content without clearing any other state or creating a second input', () => {
    const { read, patch } = rig();
    const changed = patch('first', { delivery: 'interrupt', content: [{ type: 'text', text: 'Revised' }], after_input_id: null });
    expect(changed.input).toMatchObject({ id: 'first', content: [{ type: 'text', text: 'Revised' }], delivery: 'interrupt', state_detail: 'cap_reached', queue_held: true });
    expect(changed.inputs).toEqual([changed.input]);
    expect(read().inputs).toHaveLength(2);
    expect(patch('second', { delivery: 'queue' }).input.delivery).toBe('queue');
  });

  it('rolls the entire simultaneous edit back when live steering is unsupported', () => {
    const { world, patch, read } = rig();
    world.threads.configure(ACME.mainThread, { cannot_steer: true });
    world.threads.emit(ACME.mainThread, { turns: [turn('running-queue', { ordinal: 1, state: 'running', inputId: 'accepted' })] });
    const before = structuredClone(read());
    expect(() => patch('second', { delivery: 'steer', content: [{ type: 'text', text: 'Must not be saved' }], after_input_id: null })).toThrow(Refusal);
    expect(read()).toEqual(before);
    expect(patch('second', { delivery: 'interrupt', after_input_id: null }).input.delivery).toBe('interrupt');
  });

  it('returns every renumbered neighbor as native stamped rows without losing a hold or making another input', () => {
    const { read, patch } = rig();
    const before = read().inputs;
    const changed = patch('second', { after_input_id: null });
    expect(changed.input).toMatchObject({ id: 'second', position: '1', delivery: 'queue', state: 'pending' });
    expect(changed.inputs).toHaveLength(2);
    expect(changed.inputs.find((input) => input.id === 'first')).toMatchObject({ position: '2', queue_held: true, state_detail: 'cap_reached' });
    const held = new Map(before.map((input) => [input.id, input]));
    for (const input of changed.inputs) {
      expect(input.seq_updated).toBeGreaterThan(before.find((old) => old.id === input.id)?.seq_updated ?? 0);
      held.set(input.id, input);
    }
    const order = [...held.values()].sort((one, other) => Number(one.position) - Number(other.position));
    expect(order.map((input) => input.id)).toEqual(['second', 'first']);
    expect(read().inputs).toHaveLength(2);
    expect([...held.values()]).toEqual(read().inputs);
  });

  it('refuses conflicting delivery/position and edits by another driver without leaking a partial change', () => {
    const { world, ada, patch, read } = rig();
    expect(() => patch('first', { delivery: 'queue', after_input_id: null })).toThrow(Refusal);
    const another = world.people.get('dora')!;
    const input = threadInput('dora-words', 'Dora’s original', { author: userRef(another.me), position: '3' });
    world.threads.emit(ACME.mainThread, { inputs: [input] });
    const before = structuredClone(read());
    expect(() => world.threads.handle(ada, 'PATCH', `/threads/${ACME.mainThread}/inputs/dora-words`, new URLSearchParams(), { delivery: 'interrupt', content: [{ type: 'text', text: 'Not Ada’s words' }] })).toThrow(Refusal);
    expect(read()).toEqual(before);
  });

  it('resumes only Stop holds at the reviewed revision and preserves an independent cap hold', () => {
    const { world, ada, patch, read } = rig();
    world.threads.emit(ACME.mainThread, { thread: { queue: { held: true, by: userRef(ada.me), at: '2026-10-15T12:00:00Z', held_inputs: 1, revision: 5 } } });
    const route = `/threads/${ACME.mainThread}/queue/resume`;
    expect(() => world.threads.handle(ada, 'POST', route, new URLSearchParams(), { revision: 4 })).toThrow(Refusal);
    expect(read().inputs.find((input) => input.id === 'first')?.queue_held).toBe(true);
    patch('first', { content: [{ type: 'text', text: 'Still held after editing' }] });
    expect(() => world.threads.handle(ada, 'POST', route, new URLSearchParams(), { revision: 5 })).toThrow(Refusal);
    expect(world.threads.handle(ada, 'POST', route, new URLSearchParams(), { revision: 6 })?.body).toMatchObject({ held: false, held_inputs: 0, revision: 7 });
    expect(read().inputs.find((input) => input.id === 'first')).toMatchObject({ queue_held: false, state: 'pending', state_detail: 'cap_reached' });
  });
});
