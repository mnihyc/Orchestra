// @vitest-environment node
/** Binding facts retain the full private count while sharing the existing hidden-thread observation. */
import { expect, it } from 'vitest';
import type { EverythingRowPage, ResourceImpact } from '@/generated/api-types';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { World } from '../doubles/world';

it('counts an unseen binding without naming it in the native impact facts', () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The native resource reader is required');
  const read = () => {
    const answer = world.resources.handle(person, 'POST', '/resources/impact', new URLSearchParams(), { object: { kind: 'environment', id: RUNS_ON.workspace }, model_id: null });
    expect(answer?.status).toBe(200);
    return (answer?.body as ResourceImpact).facts.dependents.find((group) => group.relation === 'binding');
  };
  const before = read();
  expect(before?.visible.some((row) => row.object?.id === ACME.repro)).toBe(true);
  world.change.control('/hide', { threads: [ACME.repro] });
  const after = read();
  expect(after?.count).toBe(before?.count);
  expect(after?.hidden).toBe(1);
  expect(after?.visible.some((row) => row.object?.id === ACME.repro)).toBe(false);
});

it('keeps name rows, table rows and full resource views separate in the native inventory', () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The native resource reader is required');
  const read = (projection: Record<string, string>) => {
    const reply = world.everything.handle(person, 'GET', '/everything', new URLSearchParams({ parent: 'resources', kinds: 'provider', ...projection }));
    expect(reply?.status).toBe(200);
    return (reply?.body as EverythingRowPage).items;
  };
  const names = read({});
  expect(names.length).toBeGreaterThan(0);
  for (const row of names) {
    expect(row.resource).toBeUndefined();
    expect(row.resource_row).toBeUndefined();
    expect(row.sharing).toBeUndefined();
  }
  for (const row of read({ resource_details: 'true' })) {
    expect(row.resource).toBeDefined();
    expect(row.resource_row).toBeUndefined();
  }
  for (const row of read({ resource_rows: 'true' })) {
    expect(row.resource).toBeUndefined();
    expect(row.resource_row?.value.list_row).toBe(true);
  }
});
