// @vitest-environment node
/** A selected role retains its source place instead of becoming an unscoped role. */
import { expect, it } from 'vitest';
import type { CapInput, UsageCap } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

for (const at of [ACME.research, null]) it(`retains ${at === null ? 'the legacy work-place default' : 'the selected role place'} in the native cap`, () => {
  const world = new World(), person = world.people.get('ada')!;
  const body: CapInput = { node_id: ACME.web, meter: 'credits', amount: '100', period: 'week', split: 'each', subject: { kind: 'role', role: 'developer', ...(at === null ? {} : { at }) } };
  const accepted = world.usage.handle(person, 'POST', '/usage/caps', new URLSearchParams(), body)!.body as UsageCap;
  expect(accepted.subject).toMatchObject({ kind: 'role', role: 'developer', ...(at === null ? {} : { at: { id: at, name: 'Research' } }) });
  if (at === null) expect(accepted.subject).not.toHaveProperty('at');
});
