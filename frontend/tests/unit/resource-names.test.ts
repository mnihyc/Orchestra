/** Only colliding resource names need readable origins; a private ancestor is never reopened. */
import { expect, it } from 'vitest';
import { resourceLabels } from '@/features/resources/labels';
import { providerNamesCollide } from '@/features/resources/providers/labels';
import { knownPlaces } from '@/lib/placeLabel';
import type { NodeRef } from '@/generated/api-types';

const acme: NodeRef = { id: 'acme', kind: 'org', name: 'Acme', redacted: false };
const beta: NodeRef = { id: 'beta', kind: 'org', name: 'Beta', redacted: false };
const known = knownPlaces([{ id: 'acme', name: 'Acme', above: [] }, { id: 'beta', name: 'Beta', above: [] }]);

it('asks for provider context only for different providers with case-folded equal names', () => {
  expect(providerNamesCollide([{ id: 'a', name: 'Shared' }, { id: 'a', name: 'SHARED' }])).toBe(false);
  expect(providerNamesCollide([{ id: 'a', name: 'Shared' }, { id: 'b', name: 'Other' }])).toBe(false);
  expect(providerNamesCollide([{ id: 'a', name: 'Shared' }, { id: 'b', name: 'shared' }])).toBe(true);
});

it('adds readable context only when two resource names collide', () => {
  const one = { id: 'one', name: 'Workshop', home: acme, org_id: acme.id, owner: null, address: 'acme.system.Workshop' };
  const two = { ...one, id: 'two', home: beta, org_id: beta.id, address: 'beta.system.Workshop' };
  const both = resourceLabels([one, two], known);
  expect(new Set(both.values()).size).toBe(2);
  expect(both.get(one.id)).toContain('Acme');
  expect(both.get(two.id)).toContain('Beta');
  expect(both.get(one.id)).not.toContain(' · ');
  for (const row of [one, two]) {
    const label = resourceLabels([row], known).get(row.id);
    expect(label).toContain('Workshop');
    expect(label).toBe('Workshop');
    expect(label).not.toContain(row.address);
  }
});

it('does not revive a redacted home from its cached name or display its protocol address', () => {
  const home = { id: 'hidden', kind: 'org' as const, name: 'Private home canary', redacted: true };
  const cached = knownPlaces([{ id: home.id, name: 'Old private canary', above: [{ id: 'root', name: 'Private root canary' }] }]);
  const row = { id: 'visible', name: 'Visible resource', home, owner: null, org_id: 'root', address: 'secret.route.Visible' };
  expect(resourceLabels([row], cached).get(row.id)).toBe('Visible resource');
});
