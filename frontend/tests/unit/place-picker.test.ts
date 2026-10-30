/** The places the settings' picker offers, with what draws each as every screen does: its colour, and whether a project is a folder. */
import { describe, expect, it } from 'vitest';

import type { TreeNode } from '@/generated/api-types';
import { placesOfTree } from '@/features/settings/PlacePicker';

const row = (id: string, kind: TreeNode['kind'], name: string, parent: string | null, more: Partial<TreeNode> = {}): TreeNode =>
  ({ id, kind, name, parent_id: parent, color: null, deleted: false, ...more }) as TreeNode;

describe('the places a picker offers', () => {
  const rows = [
    row('org', 'org', 'Acme', null, { color: 3 }),
    row('web', 'project', 'Web', 'org', { color: 5 }),
    row('shop', 'project', 'Shop', 'org'),
    row('checkout', 'project', 'Checkout', 'shop'),
    row('login', 'topic', 'Fix login', 'web'),
    row('main', 'thread', 'Main thread', 'login'),
    row('gone', 'topic', 'Gone', 'web', { deleted: true }),
  ];

  it('lists orgs, projects and topics by their paths, and leaves out a thread and what was taken away', () => {
    expect(placesOfTree(rows, ' / ').map((place) => [place.id, place.path])).toEqual([
      ['org', 'Acme'],
      ['shop', 'Acme / Shop'],
      ['checkout', 'Acme / Shop / Checkout'],
      ['web', 'Acme / Web'],
      ['login', 'Acme / Web / Fix login'],
    ]);
  });

  it('carries the colour each was given, null for the colour of its kind', () => {
    const colours = Object.fromEntries(placesOfTree(rows, ' / ').map((place) => [place.id, place.color]));
    expect(colours).toEqual({ org: 3, shop: null, checkout: null, web: 5, login: null });
  });

  it('says which project is a folder: one that lies in a project', () => {
    const folders = Object.fromEntries(placesOfTree(rows, ' / ').map((place) => [place.id, place.folder]));
    expect(folders).toEqual({ org: false, shop: false, checkout: true, web: false, login: false });
  });
  it('keeps a place’s own colour when making the picker’s summary', () => {
    const own = placesOfTree([row('org', 'org', 'Acme', null), row('web', 'project', 'Web', 'org', { custom_color: '#3a7bd5' })], ' / ');
    expect(own.find((place) => place.id === 'web')?.custom_color).toBe('#3a7bd5');
  });
});
