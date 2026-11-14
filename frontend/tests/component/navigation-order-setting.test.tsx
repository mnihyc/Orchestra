/** The order of an org's tree, set from its page: a named choice, and for an order arranged by hand each group by the names of its places, moved, taken out and added by name. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as NodesApi from '@/api/nodes';
import { SettingRow } from '@/features/settings/SettingRow';
import { withGroup } from '@/features/settings/settingsModel';
import type { ResolvedSetting } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n } from '@/i18n';
import { NavigationOrderChoice } from '@/features/settings/NavigationOrderChoice';
import { treeWriter } from '@/realtime/stores/tree';
import { treeNode } from '../doubles/tree-node';
import { show } from './support';

vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof NodesApi>(), nodeQuery: (id: string) => ({ queryKey: ['order-node', id], queryFn: () => Promise.resolve({ id: 'org', org_id: 'org', name: 'Acme', kind: 'org' }) }) }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<object>(), useOrgTreeFeed: () => undefined }));

const target = { kind: 'node', id: 'org' } as const;
beforeEach(() => {
  treeWriter.reset('org', [
    treeNode({ id: 'org', kind: 'org', parent_id: null, name: 'Acme' }),
    treeNode({ id: 'proj-a', kind: 'project', parent_id: 'org', name: 'Alpha' }),
    treeNode({ id: 'proj-b', kind: 'project', parent_id: 'org', name: 'Beta' }),
    treeNode({ id: 'topic-t', kind: 'topic', parent_id: 'proj-a', name: 'Login' }),
  ], 1);
});
afterEach(() => { cleanup(); treeWriter.clear(); });

function Draft({ initial, changed }: { initial: unknown; changed?: (value: unknown) => void }) {
  const [value, setValue] = useState<unknown>(initial);
  return <NavigationOrderChoice shape={{ type: 'json' }} label="Order of the tree" value={value} target={target} onChange={(next) => { setValue(next); changed?.(next); }} />;
}
/** The names of the places of every group, in the order they stand. */
const placed = () => [...document.querySelectorAll('[data-order-place] > span:first-child')].map((item) => item.textContent);
const group = (id: string) => {
  const found = document.querySelector<HTMLElement>(`[data-order-group="${id}"]`);
  if (found === null) throw new Error(`There is no group ${id}`);
  return within(found);
};

it('chooses how places are ordered by name, and lets go of an arrangement when another order is chosen', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft initial={{ by: 'custom', custom: { org: ['proj-b', 'proj-a'] } }} changed={changed} />);
  await user.click(screen.getByRole('combobox', { name: 'Order by' }));
  await user.click(await screen.findByRole('option', { name: 'Name' }));
  expect(changed).toHaveBeenLastCalledWith({ by: 'name' });
  expect(screen.queryByRole('list', { name: 'Arranged groups' })).not.toBeInTheDocument();
});

it('shows an arranged group by the names of its places, in order, and no identifier', async () => {
  const view = show(<Draft initial={{ by: 'custom', custom: { org: ['proj-b', 'proj-a'] } }} />);
  const acme = await view.findByText('Acme', { selector: 'span' });
  expect(acme).toBeVisible();
  expect(placed()).toEqual(['Beta', 'Alpha']);
  expect(view.container).not.toHaveTextContent('proj-');
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
});

it('moves a place up and down within its group, and not past either end', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft initial={{ by: 'custom', custom: { org: ['proj-a', 'proj-b'] } }} changed={changed} />);
  const acme = group('org');
  expect(acme.getByRole('button', { name: 'Move Alpha up in Acme' })).toBeDisabled();
  expect(acme.getByRole('button', { name: 'Move Beta down in Acme' })).toBeDisabled();
  await user.click(acme.getByRole('button', { name: 'Move Beta up in Acme' }));
  expect(changed).toHaveBeenLastCalledWith({ by: 'custom', custom: { org: ['proj-b', 'proj-a'] } });
  expect(placed()).toEqual(['Beta', 'Alpha']);
});

it('takes a place out of its group, and the group out when nothing is left in it', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft initial={{ by: 'custom', custom: { org: ['proj-a', 'proj-b'], 'proj-a': ['topic-t'] } }} changed={changed} />);
  await user.click(group('org').getByRole('button', { name: 'Remove Alpha from Acme' }));
  expect(changed).toHaveBeenLastCalledWith({ by: 'custom', custom: { org: ['proj-b'], 'proj-a': ['topic-t'] } });
  await user.click(group('proj-a').getByRole('button', { name: 'Remove Login from Alpha' }));
  expect(changed).toHaveBeenLastCalledWith({ by: 'custom', custom: { org: ['proj-b'] } });
  await user.click(group('org').getByRole('button', { name: 'Stop arranging Acme' }));
  expect(changed).toHaveBeenLastCalledWith({ by: 'custom' });
});

it('adds a place of the group by name, among those it holds that are not arranged', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft initial={{ by: 'custom', custom: { org: ['proj-a'] } }} changed={changed} />);
  // The places of the group are read once the org it is in is known.
  await user.click(await group('org').findByRole('combobox', { name: 'Add a place to Acme' }));
  expect(screen.queryByRole('option', { name: 'Alpha' })).not.toBeInTheDocument();
  await user.click(await screen.findByRole('option', { name: 'Beta' }));
  expect(changed).toHaveBeenLastCalledWith({ by: 'custom', custom: { org: ['proj-a', 'proj-b'] } });
});

it('shows a place that no longer exists as such, to be taken out', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft initial={{ by: 'custom', custom: { org: ['proj-a', 'ghost'] } }} changed={changed} />);
  expect(group('org').getByText('No longer exists')).toBeVisible();
  await user.click(group('org').getByRole('button', { name: 'Remove No longer exists from Acme' }));
  expect(changed).toHaveBeenLastCalledWith({ by: 'custom', custom: { org: ['proj-a'] } });
});

it('starts arranging a group from a list of the places that hold others, named', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft initial={{ by: 'custom' }} changed={changed} />);
  await user.click(await screen.findByRole('combobox', { name: 'Arrange a group' }));
  await user.click(await screen.findByRole('option', { name: 'Alpha' }));
  expect(changed).toHaveBeenLastCalledWith({ by: 'custom', custom: { 'proj-a': ['topic-t'] } });
});

it('keeps the raw value one press away', async () => {
  const user = userEvent.setup();
  show(<Draft initial={{ by: 'custom', custom: { org: ['proj-a'] } }} />);
  await user.click(screen.getByRole('button', { name: 'Edit as JSON' }));
  expect(screen.getByRole('textbox')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Use the form' }));
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
});


it.each(['read-only', 'locked'] as const)('shows custom navigation order by place names in a %s setting row', async (state) => {
  const def = SETTINGS_REGISTRY.find((entry) => entry.key === 'navigation.order');
  if (def === undefined) throw new Error('The registry must declare navigation order.');
  treeWriter.apply('org', [], 2, [
    { id: 'concealed-parent', name: 'Private group', kind: 'project', parent_id: 'org', owner: null, content_hidden: true, private: true },
    { id: 'hidden-child', name: 'Private topic', kind: 'topic', parent_id: 'concealed-parent', owner: null, content_hidden: true, private: true },
  ]);
  const value = { by: 'custom', custom: { org: ['proj-b', 'proj-a', 'missing-place'], 'concealed-parent': ['hidden-child'] } };
  const setting: ResolvedSetting = { key: def.key, value, own: null, source: { scope: 'org', node: null }, writable: state === 'locked', parts: [],
    locked_by: state === 'locked' ? { scope: 'system', node: null } : null };
  const view = show(<ul><SettingRow def={withGroup(def)} setting={setting} target={target} place={{ name: 'Acme', kind: 'org', owner: null, access: null }} /></ul>);
  expect(await screen.findByText('Acme', { selector: 'span' })).toBeVisible();
  expect(screen.getByText('Beta')).toBeVisible();
  expect(screen.getByText('Alpha')).toBeVisible();
  expect(screen.getByText('No longer exists')).toBeVisible();
  expect(screen.getAllByText(i18n.t('access.place.hidden'))).toHaveLength(2);
  for (const id of ['proj-b', 'proj-a', 'missing-place', 'concealed-parent', 'hidden-child']) expect(view.container).not.toHaveTextContent(id);
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
});
