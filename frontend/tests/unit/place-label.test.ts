/** A place's label: its name alone while no other place the reader sees has it, and with where it is, in as few words as tell them apart, once one does. */
import { describe, expect, it } from 'vitest';

import { knownPlaces, type Placed, placeLabel, placeLabels, placedOfPath } from '@/lib/placeLabel';

const at = (id: string, name: string, ...above: string[]): Placed => ({ id, name, above: above.map((place) => ({ id: `${place}-id`, name: place })) });

/** One person in Acme and Beta, each with a department dep2 holding a project web and its topic login. */
const world = [
  at('acme', 'Acme'),
  at('beta', 'Beta'),
  at('acme-dep2', 'dep2', 'Acme'),
  at('beta-dep2', 'dep2', 'Beta'),
  at('acme-web', 'web', 'Acme', 'dep2'),
  at('beta-web', 'web', 'Beta', 'dep2'),
  at('acme-login', 'login', 'Acme', 'dep2', 'web'),
  at('beta-login', 'login', 'Beta', 'dep2', 'web'),
  at('acme-infra', 'Infra', 'Acme'),
];
const known = knownPlaces(world);
const label = (id: string) => placeLabel(world.find((place) => place.id === id)!, known);

describe('a place’s label', () => {
  it('is its name alone where no other place the reader sees has it', () => {
    expect(label('acme-infra')).toBe('Infra');
    expect(label('acme')).toBe('Acme');
  });

  it('says the org where a place of the same name lies in another org', () => {
    expect(label('acme-dep2')).toBe('dep2 · Acme');
    expect(label('beta-dep2')).toBe('dep2 · Beta');
    expect(label('acme-web')).toBe('web · Acme');
    expect(label('beta-login')).toBe('login · Beta');
  });

  it('says the place it lies in where the org does not tell them apart, and both where neither alone does', () => {
    const places = [at('one', 'web', 'Acme', 'dep1'), at('two', 'web', 'Acme', 'dep2'), at('three', 'web', 'Acme'), at('four', 'web', 'Beta', 'dep1')];
    const labels = placeLabels(places);
    expect(labels.get('one')).toBe('web · dep1 · Acme');
    expect(labels.get('two')).toBe('web · dep2');
    expect(labels.get('three')).toBe('web · Acme');
    expect(labels.get('four')).toBe('web · Beta');
    // Never two lines alike.
    expect(new Set(labels.values()).size).toBe(places.length);
  });

  it('tells two root orgs of one name apart by their addresses', () => {
    const orgs: Placed[] = [{ id: 'a', name: 'Acme', above: [], address: 'acme' }, { id: 'b', name: 'Acme', above: [], address: 'acme-eu' }];
    const labels = placeLabels(orgs);
    expect(labels.get('a')).toBe('Acme · acme');
    expect(labels.get('b')).toBe('Acme · acme-eu');
  });

  it('reads two names that differ in case or spaces alone as one name', () => {
    const labels = placeLabels([at('x', 'Dep2', 'Acme'), at('y', 'dep2 ', 'Beta')]);
    expect(labels.get('x')).toBe('Dep2 · Acme');
    expect(labels.get('y')).toBe('dep2  · Beta');
  });

  it('tells the rows of one list apart even where nothing else is known of them', () => {
    const hits = [placedOfPath([{ id: 'acme', name: 'Acme' }, { id: 'w1', name: 'web' }]), placedOfPath([{ id: 'beta', name: 'Beta' }, { id: 'w2', name: 'web' }])].filter((place): place is Placed => place !== null);
    const labels = placeLabels(hits);
    expect(labels.get('w1')).toBe('web · Acme');
    expect(labels.get('w2')).toBe('web · Beta');
  });

  it('names nothing of a place above that the reader may not see', () => {
    const labels = placeLabels([
      { id: 'p', name: 'web', above: [{ id: 'acme', name: 'Acme' }, { id: 'secret', name: null }] },
      { id: 'q', name: 'web', above: [{ id: 'beta', name: 'Beta' }, { id: 'open', name: 'dep2' }] },
    ]);
    expect(labels.get('p')).toBe('web · Acme');
    expect(labels.get('q')).toBe('web · Beta');
  });
});
