// @vitest-environment node
/** Native document destination observations neither change the source nor grant its later write. */
import { expect, it } from 'vitest';
import type { CopyDestinationsRequest, CopyDestinationsResponse } from '@/generated/api-types';
import { ACME, id } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';

function scene(username = 'ada', title = 'Rollout plan') {
  const world = new World(), person = world.people.get(username);
  const doc = [...world.documents.docs.values()].find((value) => value.title === title);
  if (person === undefined || doc === undefined) throw new Error('The native document scene is required');
  const request: CopyDestinationsRequest = { subject: { kind: 'document', id: doc.id }, intent: 'move', destinations: [{ kind: 'node', id: ACME.upgradeDeps }] };
  const judge = (body = request) => {
    const answer = world.documents.handle(person, 'POST', '/copy-destinations/judge', new URLSearchParams(), body);
    expect(answer?.status).toBe(200);
    return answer?.body as CopyDestinationsResponse;
  };
  return { world, person, doc, request, judge };
}

it('observes an actual same-org destination without changing identity, location or content', () => {
  const { doc, request, judge } = scene();
  const before = structuredClone(doc);
  expect(judge()).toEqual({ subject: request.subject, intent: 'move', source_owned: true,
    destinations: [{ destination: request.destinations[0], cross_root: false, outcome: { state: 'needs_choice', choice: 'in_org_move' } }] });
  expect(doc).toEqual(before);
});

it('leaves the native Manage requirement intact after a reader observes the destination', () => {
  const { world, person, doc, judge } = scene('vera');
  expect(judge().destinations[0]?.outcome).toEqual({ state: 'needs_choice', choice: 'in_org_move' });
  const before = doc.nodeId;
  expect(() => world.documents.handle(person, 'POST', `/documents/${doc.id}/move`, new URLSearchParams(), { node_id: ACME.upgradeDeps })).toThrow(Refusal);
  expect(doc.nodeId).toBe(before);
});

it('reports an unavailable destination without inventing a readable node or a creation grant', () => {
  const { request, judge } = scene();
  request.destinations = [{ kind: 'node', id: id(3, 9999) }];
  const result = judge().destinations[0];
  expect(result).toMatchObject({ destination: request.destinations[0], cross_root: false, outcome: { state: 'blocked', cause: 'not_available' } });
  if (result?.outcome.state !== 'blocked') throw new Error('The missing destination must stay blocked');
  expect(result.outcome.problem.status).toBe(404);
});

it('retains the actual ownership boundary for a cross-org move', () => {
  const { request, judge } = scene('ada', 'Session notes');
  request.destinations = [{ kind: 'system' }];
  expect(judge()).toMatchObject({ source_owned: false, destinations: [{ cross_root: true, outcome: { state: 'blocked', cause: 'owners_only' } }] });
});

it('does not reveal a private source to another person', () => {
  const { judge } = scene('dora', 'Only for me');
  try { judge(); expect.fail('An unreadable private source must be refused'); }
  catch (error) {
    expect(error).toBeInstanceOf(Refusal);
    if (!(error instanceof Refusal)) throw error;
    expect(error.answer.status).toBe(404);
  }
});

it('keeps the native bound on one explicit destination page', () => {
  const { request, judge } = scene();
  request.destinations = Array.from({ length: 101 }, () => ({ kind: 'node', id: ACME.upgradeDeps }));
  expect(() => judge()).toThrow(Refusal);
});
