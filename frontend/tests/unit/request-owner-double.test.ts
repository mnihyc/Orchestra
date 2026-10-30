// @vitest-environment node
/** Access requests belong to a person across request/session projections, never to a JavaScript object. */
import { expect, it } from 'vitest';
import type { Approval, AuthResult } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';

function fixture() {
  const world = new World();
  const approval = world.notifications.control('/approval/access', {
    requester: 'dora', batch_id: 'owned-batch', object: { kind: 'thread', id: ACME.mainThread, name: 'Main thread' }, permissions: ['thread.view'],
  })!.body as Approval;
  const auth = world.login({ login: 'dora', password: 'compilers and goto' }).body as AuthResult;
  const actor = () => world.browserPerson(`Bearer ${auth.access_token}`);
  return { world, approval, actor };
}

it('lists the person’s requested approvals through a fresh browser projection', () => {
  const { world, approval, actor } = fixture();
  expect(world.access.requests.handle(actor(), 'GET', '/approvals', new URLSearchParams({ scope: 'requested' }), {})?.body)
    .toMatchObject({ items: [{ id: approval.id }] });
});

it('reads the person’s batch through a fresh browser projection', () => {
  const { world, approval, actor } = fixture();
  expect(world.access.requests.handle(actor(), 'GET', '/access-requests/owned-batch', new URLSearchParams(), {})?.body)
    .toMatchObject({ batch_id: 'owned-batch', approvals: [{ id: approval.id }] });
});

it('lets the same person cancel through a fresh browser projection', () => {
  const { world, approval, actor } = fixture();
  expect(world.access.requests.handle(actor(), 'POST', `/approvals/${approval.id}/cancel`, new URLSearchParams(), {})?.body)
    .toMatchObject({ id: approval.id, state: 'cancelled' });
});

it('counts the same person’s pending requests against their limit through a fresh projection', () => {
  const { world, actor } = fixture();
  world.access.requests.control('/limit', { waiting: 1 });
  expect(() => world.access.requests.handle(actor(), 'POST', '/access-requests', new URLSearchParams(), {
    items: [{ object: { kind: 'thread', id: ACME.mainThread }, permissions: ['access.own'] }],
  })).toThrow('access.too_many_requests');
});

it('does not let another person read that batch or cancel its request', () => {
  const { world, approval } = fixture();
  const other = world.people.get('ada')!;
  expect(() => world.access.requests.handle(other, 'GET', '/access-requests/owned-batch', new URLSearchParams(), {})).toThrow(Refusal);
  expect(() => world.access.requests.handle(other, 'POST', `/approvals/${approval.id}/cancel`, new URLSearchParams(), {})).toThrow(Refusal);
  expect(world.access.requests.recordOf(approval.id)?.approval.state).toBe('pending');
});
