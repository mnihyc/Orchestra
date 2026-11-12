/** Both native approval suppliers retain authorized history while settled requests offer no decision. */
import { expect, it } from 'vitest';
import type { Approval, ApprovalPage } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World, type Person } from '../doubles/world';

for (const source of ['general', 'access-only'] as const) for (const state of ['approved', 'denied'] as const) it(`retains ${state} access history through ${source} without widening its audience`, () => {
  const world = new World();
  const ada = world.people.get('ada'), grace = world.people.get('grace'), vera = world.people.get('vera'), dora = world.people.get('dora');
  if (ada === undefined || grace === undefined || vera === undefined || dora === undefined) throw new Error('The existing fixture people must be present');
  const made = world.notifications.control('/approval/access', { requester: 'grace', title: 'Review the plan', deciders: ['ada'], also: ['dora'],
    permissions: ['thread.view'], object: { id: ACME.mainThread, kind: 'thread', name: 'Main thread' } });
  if (made?.status !== 200) throw new Error('The native access request must be created');
  const request = made.body as Approval;
  const read = (person: Person, scope: 'mine' | 'requested' | 'others', status: 'pending' | 'decided', all = false) => {
    const query = new URLSearchParams({ scope, state: status, ...(all ? { all: 'true' } : {}) });
    const answer = source === 'general' ? world.notifications.handle(person, 'GET', '/approvals', query, {})
      : world.access.requests.handle(person, 'GET', '/approvals', query, {});
    if (answer?.status !== 200) throw new Error('The native approval supplier must return its page');
    return (answer.body as ApprovalPage).items.filter((row) => row.id === request.id);
  };
  expect(read(ada, 'mine', 'pending')).toMatchObject([{ id: request.id, can_decide: true }]);
  world.notifications.settleAs(request.id, 'ada', state, { approve: state === 'approved' });
  expect(read(ada, 'mine', 'pending')).toEqual([]);
  expect(read(ada, 'mine', 'decided')).toMatchObject([{ id: request.id, state, can_decide: false }]);
  expect(read(grace, 'requested', 'decided')).toMatchObject([{ id: request.id, state, can_decide: false }]);
  expect(read(vera, 'mine', 'decided')).toEqual([]);
  expect(read(dora, 'mine', 'decided')).toEqual([]);
  expect(read(dora, 'mine', 'decided', true)).toMatchObject([{ id: request.id, state, can_decide: false }]);
  expect(read(dora, 'others', 'decided')).toMatchObject([{ id: request.id, state, can_decide: false }]);
});
