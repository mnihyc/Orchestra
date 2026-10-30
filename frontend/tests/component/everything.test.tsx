/** The complete tree grid as a reader uses it: remembered columns, server sorting, keyboard branches and minimal paths. */
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { EverythingFilter } from '@/api/everything';
import type * as SearchApi from '@/api/search';
import type * as Screen from '@/shell/screen';
import { EverythingBrowser, EverythingScreen } from '@/features/everything/Everything';
import { KindIcon } from '@/features/everything/EverythingTable';
import { OrgEverything } from '@/features/settings/org/Everything';
import type { EverythingRow, Me } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';

import { inventoryRow } from '../doubles/everything-row';
import { display } from '../doubles/thread-display';
import { CATALOGUE } from '../doubles/catalogue';
import { me, org, userRef } from '../doubles/fixtures';
import { show } from './support';
import { retiredCopy } from './retired-copy';

let reader: Me = me(1, 'reader', 'Reader');
let adminMode = false;
let levels = new Map<string | null, EverythingRow[]>();
const reads: { parent: string | null; filter: EverythingFilter }[] = [];
const openGuide = vi.hoisted(() => vi.fn());
const namesRead = vi.hoisted(() => vi.fn());
const shareMany = vi.hoisted(() => vi.fn());
vi.mock('@/api/auth', () => ({ meQuery: { queryKey: ['inventory-reader'], initialData: () => reader, queryFn: () => Promise.resolve(reader) } }));
vi.mock('@/api/search', async (original) => ({ ...await original<typeof SearchApi>(), namesQuery: (...args: Parameters<typeof SearchApi.namesQuery>) => ({
  queryKey: ['inventory-owner', ...args], queryFn: () => { namesRead(...args); return Promise.resolve([]); },
}) }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/features/access/AdminMode', () => ({ useAdminMode: () => ({ data: { as_admin: adminMode, available: true } }) }));
vi.mock('@/api/everything', () => ({ everythingQuery: (person: string, parent: string | null, filter: EverythingFilter) => ({
  queryKey: ['everything', person, parent, filter], initialPageParam: null,
  // These cases exercise a loaded hierarchy; identity and fixture rows need no scheduling race before the first action.
  initialData: () => ({ pages: [{ items: levels.get(parent) ?? [], next_cursor: null }], pageParams: [null] }),
  queryFn: () => { reads.push({ parent, filter }); return Promise.resolve({ items: levels.get(parent) ?? [], next_cursor: null }); },
  getNextPageParam: () => undefined,
}) }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: vi.fn(), openAccessToMany: shareMany }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => openGuide }));

const root = inventoryRow({ id: 'acme', kind: 'org', name: 'Acme', href: '/org/acme', has_children: true });
const project = inventoryRow({ id: 'web', kind: 'project', name: 'Web', parent_id: 'acme', place_label: 'Acme', href: '/project/web', has_children: true, owner: userRef(reader), created_at: '2026-10-08T10:00:00.000Z' });
const leaf = inventoryRow({ id: 'release', kind: 'topic', name: 'Release plan', parent_id: 'web', href: '/topic/release', place_label: 'Acme / Web' });
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearanceBefore = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');

beforeEach(() => {
  reader = me(1, 'reader', 'Reader');
  adminMode = false;
  localStorage.clear(); reads.length = 0;
  openGuide.mockClear(); namesRead.mockClear(); shareMany.mockClear();
  levels = new Map([[null, [root]], ['acme', [project]], ['web', [leaf]]]);
});
afterEach(async () => {
  cleanup(); useAppearance.setState(appearanceBefore); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});

describe('the complete hierarchy', () => {
  it.each(modes)('leads with the list and mounts its filters only on request in %s/%s', async (locale, theme) => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup();
    const view = show(<EverythingScreen />);
    expect(await screen.findByRole('treegrid', { name: i18n.t('everything.tree') })).toBeVisible();
    expect(screen.getAllByRole('heading', { name: i18n.t('everything.title'), level: 1 })).toHaveLength(1);
    expect(screen.queryByText(retiredCopy(locale, 'everything.lead'))).not.toBeInTheDocument();
    expect(screen.queryByText(retiredCopy(locale, 'everything.keyboard'))).not.toBeInTheDocument();
    expect(screen.getAllByRole('searchbox')).toHaveLength(1);
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
    expect(screen.queryByRole('switch')).not.toBeInTheDocument();
    expect(namesRead).not.toHaveBeenCalled();
    const filters = screen.getByRole('button', { name: i18n.t('everything.filters') });
    expect(filters).toHaveAttribute('aria-expanded', 'false');
    await user.click(filters);
    const panel = screen.getByRole('dialog', { name: i18n.t('everything.filters') });
    expect(within(panel).getByRole('combobox', { name: i18n.t('everything.columns.owner') })).toBeVisible();
    expect(within(panel).getByLabelText(i18n.t('everything.createdFrom'))).toBeVisible();
    const hideAdmin = within(panel).getByRole('checkbox', { name: i18n.t('inventoryChoices.everything.hideAdmin.name') });
    await user.click(hideAdmin);
    await waitFor(() => expect(reads.at(-1)?.filter.adminAccess).toBe(false));
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: i18n.t('everything.filters') })).not.toBeInTheDocument();
    expect(filters).toHaveTextContent(i18n.t('everything.activeFilters', { count: 1 }));
    expect(filters).toHaveFocus();
    expect(namesRead).not.toHaveBeenCalled();
    await user.click(filters);
    expect(screen.getByRole('checkbox', { name: i18n.t('inventoryChoices.everything.hideAdmin.name') })).toBeChecked();
    await user.click(screen.getByRole('button', { name: i18n.t('everything.clearFilters') }));
    await waitFor(() => expect(reads.at(-1)?.filter.adminAccess).toBeUndefined());
    expect(filters).toHaveTextContent(i18n.t('everything.filters'));
    view.unmount();
    show(<OrgEverything org={org('acme', 'acme', 'Acme', userRef(reader))} />);
    expect(await screen.findByRole('link', { name: 'Web · Acme' })).toBeVisible();
    expect(screen.getAllByRole('heading', { name: i18n.t('everything.title') })).toHaveLength(1);
    expect(screen.queryByText(retiredCopy(locale, 'everything.lead'))).not.toBeInTheDocument();
    await waitFor(() => expect(reads.some((read) => read.parent === 'acme' && read.filter.scope === 'mine')).toBe(true));
  });

  it('renders an icon for every ObjectKind, conversations, groups and a future kind', () => {
    const kinds = new Set<string>(CATALOGUE.permissions.flatMap((permission) => permission.on));
    for (const kind of ['org', 'project', 'topic', 'thread', 'document', 'provider', 'agent', 'environment', 'host', 'machine', 'template', 'mcp_server', 'mount', 'chat', 'group', 'a_future_kind']) kinds.add(kind);
    for (const kind of kinds) {
      const icon = render(<KindIcon row={{ id: kind, kind, path: [] }} />);
      expect(icon.container.querySelector('svg'), kind).toBeInTheDocument();
      icon.unmount();
    }
  });

  it.each(['waiting_approval', 'blocked', 'failed', 'running', 'queued', 'held', 'finished', 'idle', 'archived'] as const)('keeps kind, name and the supplied %s activity in a Recent card’s first line', async (state) => {
    levels.set(null, [inventoryRow({ id: 'recent-thread', kind: 'thread', name: 'Recent thread', href: '/thread/recent-thread',
      last_opened: new Date().toISOString(), state: 'idle', thread_display: display(state, { queued_inputs: 2, held_inputs: 2 }) })]);
    const view = show(<EverythingBrowser view={{ arrangement: 'recent', order: 'opened' }} />);
    const row = (await screen.findByRole('link', { name: 'Recent thread' })).closest('tr');
    if (row === null) throw new Error('A recent item needs its card row');
    expect(row.querySelector('.flat-name [data-node-icon="thread"]')).toBeInTheDocument();
    expect(row.querySelector(`.flat-state [data-work-state="${state}"]`)).toBeInTheDocument();
    expect(row.querySelectorAll('[data-work-state]')).toHaveLength(1);
    expect(view.container.querySelector('a[data-open]')).toHaveTextContent('Recent thread');
  });

  it('expands Resources and Expand all without crashing on any resource kind', async () => {
    const user = userEvent.setup();
    levels.set(null, [inventoryRow({ id: 'resources', kind: 'group', group: 'resources', name: 'resources', has_children: true })]);
    levels.set('resources', (['environment', 'provider', 'agent', 'mcp_server', 'machine', 'host', 'mount', 'template'] as const)
      .map((kind) => inventoryRow({ id: kind, kind, name: `${kind} record`, parent_id: 'resources', href: `/resources/${kind}` })));
    show(<EverythingBrowser />);
    await user.click(await screen.findByRole('button', { name: 'Expand Resources' }));
    expect(await screen.findByRole('link', { name: 'agent record' })).toBeVisible();
    expect(screen.getByRole('link', { name: 'mcp_server record' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Collapse all' }));
    expect(screen.queryByRole('link', { name: 'agent record' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Expand all' }));
    expect(await screen.findByRole('link', { name: 'mcp_server record' })).toBeVisible();
    expect(screen.queryByText('This screen could not be shown')).not.toBeInTheDocument();
  });

  it('offers Created and keeps the remembered admin column only while the session switch is on', async () => {
    const user = userEvent.setup();
    const view = show(<EverythingBrowser />);
    await screen.findByRole('link', { name: 'Acme' });
    expect(screen.queryByRole('columnheader', { name: 'As admin' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Columns' }));
    expect(screen.queryByRole('menuitemcheckbox', { name: 'As admin' })).not.toBeInTheDocument();
    expect(screen.getByRole('menuitemcheckbox', { name: 'Created' })).toBeVisible();
    expect(screen.getByText('When the item was made.')).toBeVisible();
    await user.click(screen.getByRole('menuitemcheckbox', { name: 'Created' }));
    expect(screen.getByRole('columnheader', { name: 'Created' })).toBeVisible();
    adminMode = true;
    view.rerender(<EverythingBrowser />);
    expect(await screen.findByRole('columnheader', { name: 'As admin' })).toBeVisible();
    adminMode = false;
    view.rerender(<EverythingBrowser />);
    expect(screen.queryByRole('columnheader', { name: 'As admin' })).not.toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'Created' })).toBeVisible();
  });

  it('keeps one participation choice and one independent hide-admin choice', async () => {
    const user = userEvent.setup();
    show(<EverythingBrowser />);
    await screen.findByRole('link', { name: 'Acme' });
    await user.click(screen.getByRole('button', { name: 'Filters' }));
    const participation = screen.getByRole('switch', { name: 'Everything I can see' });
    const ordinary = screen.getByRole('checkbox', { name: 'Hide admin-only' });
    expect(screen.getAllByRole('switch', { name: 'Everything I can see' })).toHaveLength(1);
    expect(screen.getAllByRole('checkbox', { name: 'Hide admin-only' })).toHaveLength(1);
    expect(participation).not.toHaveAccessibleDescription();
    await user.click(participation);
    await user.click(ordinary);
    expect(participation).toBeChecked();
    expect(participation).toHaveAccessibleName('Only where I take part');
    expect(ordinary).toBeChecked();
    await waitFor(() => { expect(reads.at(-1)?.filter).toMatchObject({ participating: true, adminAccess: false }); });
    await user.click(ordinary);
    expect(participation).toBeChecked();
    await waitFor(() => { expect(reads.at(-1)?.filter.adminAccess).toBeUndefined(); });
  });

  it('keeps named kind and column choices with explanations in their open menus and no per-option guide', async () => {
    const user = userEvent.setup();
    show(<EverythingBrowser />);
    await screen.findByRole('link', { name: 'Acme' });
    await user.click(screen.getByRole('button', { name: 'Filters' }));
    await user.click(screen.getByRole('button', { name: 'All kinds' }));
    expect(screen.getByRole('menuitemcheckbox', { name: /^Environment/ })).toBeVisible();
    expect(screen.getByText('The files, processes and tools where work runs.')).toBeVisible();
    // Descriptions belong inside an open picker, without a separate guide action for each option.
    expect(screen.queryByRole('menuitem', { name: 'Read about Environment' })).not.toBeInTheDocument();
    expect(openGuide).not.toHaveBeenCalled();
    expect(reads.at(-1)?.filter.kinds).toEqual([]);
    await user.click(screen.getByRole('menuitemcheckbox', { name: /^Environment/ }));
    await waitFor(() => { expect(reads.at(-1)?.filter.kinds).toEqual(['environment']); });
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('button', { name: 'Columns' }));
    expect(screen.getByRole('menuitemcheckbox', { name: /^My access/ })).toBeVisible();
    expect(screen.getByText('What you may do here, including steps shown only as a path.')).toBeVisible();
  });

  it('offers Mine and System as short scope names with their explanations', async () => {
    const user = userEvent.setup();
    reader = { ...reader, is_system_admin: true };
    adminMode = true;
    show(<EverythingBrowser onScope={vi.fn()} />);
    await screen.findByRole('link', { name: 'Acme' });
    await user.click(screen.getByRole('button', { name: 'Filters' }));
    await user.click(screen.getByRole('combobox', { name: 'Scope' }));
    expect(screen.getByRole('option', { name: /^Mine/ })).toBeVisible();
    expect(screen.getAllByText('Your orgs, your own place, shared items and resources you can see.')[0]).toBeVisible();
    expect(screen.getByRole('option', { name: /^System/ })).toBeVisible();
    expect(screen.getAllByText('Every org and system resource; requires a system administrator.')[0]).toBeVisible();
  });

  it('does not offer system scope from administrator standing while the session switch is off', async () => {
    reader = { ...reader, is_system_admin: true };
    show(<EverythingBrowser onScope={vi.fn()} />);
    await screen.findByRole('link', { name: 'Acme' });
    await userEvent.click(screen.getByRole('button', { name: 'Filters' }));
    expect(screen.queryByRole('combobox', { name: 'Scope' })).not.toBeInTheDocument();
  });

  it('renders the agent-profile and MCP-server kinds with their names and working row icons', async () => {
    levels.set(null, [inventoryRow({ id: 'profile', kind: 'agent', name: 'Policy agent' }), inventoryRow({ id: 'tools', kind: 'mcp_server', name: 'Team tools' })]);
    show(<EverythingBrowser />);
    const agent = await screen.findByRole('row', { name: /Policy agent/ });
    const mcp = await screen.findByRole('row', { name: /Team tools/ });
    expect(within(agent).getByText('Agent profile')).toBeVisible();
    expect(within(mcp).getByText('MCP server')).toBeVisible();
    expect(agent.querySelector('svg')).not.toBeNull();
    expect(mcp.querySelector('svg')).not.toBeNull();
  });

  it('opens branches with chevrons and the tree-grid arrow keys, and collapses all', async () => {
    const user = userEvent.setup();
    show(<EverythingBrowser />);
    const tree = await screen.findByRole('treegrid', { name: 'Everything I can see' });
    const first = await within(tree).findByRole('row', { name: /Acme/ });
    first.focus();
    await user.keyboard('{ArrowRight}');
    const web = await within(tree).findByRole('link', { name: 'Web · Acme' });
    expect(web.closest('tr')).toHaveAttribute('aria-level', '2');
    await user.keyboard('{ArrowRight}');
    expect(web.closest('tr')).toHaveFocus();
    await user.keyboard('{ArrowRight}');
    expect(await within(tree).findByRole('link', { name: 'Release plan · Acme / Web' })).toBeVisible();
    await user.keyboard('{ArrowLeft}');
    expect(within(tree).queryByRole('link', { name: /Release plan/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Expand all' }));
    expect(await screen.findByRole('link', { name: /Release plan/ })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Collapse all' }));
    expect(screen.queryByRole('link', { name: /Web/ })).not.toBeInTheDocument();
  });

  it('keeps Name, remembers optional columns for this person, and keeps another person’s choice separate', async () => {
    const user = userEvent.setup();
    const view = show(<EverythingBrowser />);
    await screen.findByRole('link', { name: 'Acme' });
    await user.click(screen.getByRole('button', { name: 'Columns' }));
    expect(screen.queryByRole('menuitemcheckbox', { name: 'Name' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('menuitemcheckbox', { name: 'Kind' }));
    expect(screen.queryByRole('columnheader', { name: 'Kind' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Columns' }));
    await user.click(screen.getByRole('menuitemcheckbox', { name: 'Created' }));
    expect(screen.getByRole('columnheader', { name: 'Created' })).toBeVisible();
    view.unmount();
    const again = show(<EverythingBrowser />);
    await screen.findByRole('link', { name: 'Acme' });
    expect(screen.getByRole('columnheader', { name: 'Name' })).toBeVisible();
    expect(screen.getByRole('columnheader', { name: 'Created' })).toBeVisible();
    expect(screen.queryByRole('columnheader', { name: 'Kind' })).not.toBeInTheDocument();
    again.unmount();
    reader = me(2, 'another', 'Another reader');
    show(<EverythingBrowser />);
    expect(await screen.findByRole('columnheader', { name: 'Kind' })).toBeVisible();
    expect(screen.queryByRole('columnheader', { name: 'Created' })).not.toBeInTheDocument();
  });

  it('asks the server to sort each open level and announces it without a duplicate visible row', async () => {
    const user = userEvent.setup();
    show(<EverythingBrowser />);
    await user.click(await screen.findByRole('button', { name: 'Expand Acme' }));
    await screen.findByRole('link', { name: /Web/ });
    await user.click(screen.getByRole('button', { name: 'Name' }));
    await waitFor(() => expect(reads.some((read) => read.parent === 'acme' && read.filter.sort === 'name' && read.filter.direction === 'desc')).toBe(true));
    expect(screen.getByRole('columnheader', { name: 'Name' })).toHaveAttribute('aria-sort', 'descending');
    expect(within(screen.getByRole('group', { name: 'Sort' })).getByText('Sort: Name, descending')).toHaveClass('sr-only');
    expect(screen.getByRole('button', { name: 'Filters' })).toHaveAttribute('aria-expanded', 'false');
  });

  it('keeps path-only rows expandable while withholding metadata, links and actions', async () => {
    const user = userEvent.setup();
    const hidden = inventoryRow({ id: 'hidden', kind: 'project', name: null, parent_id: 'acme', has_children: true, my_access: 'path_only', created_at: null });
    levels.set('acme', [hidden]); levels.set('hidden', [inventoryRow({ ...leaf, parent_id: 'hidden' })]);
    show(<EverythingBrowser />);
    await user.click(await screen.findByRole('button', { name: 'Expand Acme' }));
    const row = (await screen.findByText('Private place')).closest('tr');
    if (row === null) throw new Error('The path step needs a table row.');
    expect(within(row).getByText('Path only')).toBeVisible();
    expect(within(row).queryByRole('link')).not.toBeInTheDocument();
    expect(within(row).queryByRole('button', { name: /Actions/ })).not.toBeInTheDocument();
    expect(within(row).getAllByRole('gridcell')[1]).toHaveTextContent('Project');
    expect(within(row).getAllByRole('gridcell').slice(2).every((cell) => cell.textContent === '')).toBe(true);
    await user.click(within(row).getByRole('button', { name: 'Expand Private place' }));
    expect(await screen.findByRole('link', { name: /Release plan/ })).toBeVisible();
  });

  it('filters by the owner named on a row even when that person is outside the ordinary directory', async () => {
    const user = userEvent.setup();
    show(<EverythingBrowser />);
    await user.click(await screen.findByRole('button', { name: 'Expand Acme' }));
    await user.click(await screen.findByRole('button', { name: i18n.t('everything.onlyOwner', { name: 'Reader' }) }));
    await waitFor(() => expect(reads.some((read) => read.filter.owner === reader.id)).toBe(true));
    expect(screen.queryByRole('combobox', { name: 'Owner' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Filters (1)' }));
    expect(screen.getByRole('combobox', { name: 'Owner' })).toHaveValue('Reader');
    await user.click(screen.getByRole('button', { name: 'Clear filters' }));
    await waitFor(() => expect(reads.at(-1)?.filter.owner).toBeNull());
  });

  it('names the org beside repeated names and offers only the returned row actions', async () => {
    const user = userEvent.setup();
    levels.set(null, [inventoryRow({ id: 'dep-a', kind: 'org', name: 'dep2', place_label: 'Acme', href: '/org/dep-a', can_share: true }), inventoryRow({ id: 'dep-b', kind: 'org', name: 'dep2', place_label: 'Beta', href: '/org/dep-b', settings_href: '/org/dep-b/settings/general' })]);
    show(<EverythingBrowser />);
    expect(await screen.findByRole('link', { name: 'dep2 · Acme' })).toBeVisible();
    expect(screen.getByRole('link', { name: 'dep2 · Beta' })).toBeVisible();
    // jsdom has no layout; the native detached-anchor check needs this visible scroll boundary.
    const viewport = screen.getByRole('region', { name: i18n.t('everything.tree') });
    for (const element of [viewport, viewport.parentElement]) {
      if (element === null) throw new Error('The hierarchy needs its scroll viewport.');
      vi.spyOn(element, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, window.innerWidth, window.innerHeight));
      Object.defineProperties(element, {
        clientWidth: { configurable: true, value: window.innerWidth },
        clientHeight: { configurable: true, value: window.innerHeight },
      });
    }
    const rows = screen.getAllByRole('row').filter((row) => row.textContent.includes('dep2'));
    await user.click(within(rows[0]!).getByRole('button', { name: 'Actions for dep2' }));
    expect(screen.getByRole('menuitem', { name: 'Share' })).toBeVisible();
    expect(screen.queryByRole('menuitem', { name: 'Place settings' })).not.toBeInTheDocument();
    await user.keyboard('{Escape}');
    await user.click(within(rows[1]!).getByRole('button', { name: 'Actions for dep2' }));
    expect(screen.getByRole('menuitem', { name: 'Place settings' })).toHaveAttribute('href', '/org/dep-b/settings/general');
    expect(screen.queryByRole('menuitem', { name: 'Share' })).not.toBeInTheDocument();
  });

  it('switches to flat Recent, selects Active and remembers its order and filters on reopening', async () => {
    const user = userEvent.setup();
    levels.set(null, [inventoryRow({ id: 'document', kind: 'document', name: 'Notes', href: '/doc/document', last_opened: new Date().toISOString(), last_activity: new Date().toISOString() })]);
    const view = show(<EverythingBrowser />);
    await user.click(await screen.findByRole('button', { name: 'Recent' }));
    expect(screen.getByRole('button', { name: 'Recent' })).toHaveAttribute('aria-pressed', 'true');
    expect(await screen.findByRole('columnheader', { name: 'Where' })).toBeVisible();
    expect(screen.getByRole('columnheader', { name: 'Opened' })).toHaveAttribute('aria-sort', 'descending');
    expect(screen.getByRole('rowheader', { name: 'Today' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Expand all' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('combobox', { name: 'Recent order' }));
    await user.click(screen.getByRole('option', { name: 'Active' }));
    await waitFor(() => expect(reads.at(-1)?.filter).toMatchObject({ recent: 'active', sort: 'last_activity', direction: 'desc' }));
    expect(screen.getByRole('columnheader', { name: 'Active' })).toHaveAttribute('aria-sort', 'descending');
    expect(screen.getByRole('columnheader', { name: 'By' })).toBeVisible();
    view.unmount();
    show(<EverythingBrowser />);
    expect(await screen.findByRole('combobox', { name: 'Recent order' })).toHaveTextContent('Active');
    expect(screen.getByRole('button', { name: 'Recent' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('carries a narrowed beginning and its kind, archive and done choices in one cross-level request', async () => {
    show(<EverythingBrowser parent="web" view={{ arrangement: 'recent', order: 'opened', kinds: ['document'], navigation: true, showArchived: false, hideDone: true }} />);
    await screen.findByRole('treegrid');
    await waitFor(() => expect(reads.at(-1)).toMatchObject({ parent: 'web', filter: { recent: 'opened', kinds: ['document'], navigation: true, showArchived: false, hideDone: true } }));
    expect(reads.every((read) => read.parent === 'web')).toBe(true);
  });

  it('selects permitted rows across levels and opens the existing multi-item Share review without detail reads', async () => {
    const user = userEvent.setup();
    levels.set(null, [inventoryRow({ id: 'one', kind: 'document', name: 'One', can_share: true }), inventoryRow({ id: 'two', kind: 'topic', name: 'Two', can_share: true }), inventoryRow({ id: 'path', kind: 'project', name: null, my_access: 'path_only' })]);
    show(<EverythingBrowser view={{ arrangement: 'recent', order: 'opened' }} />);
    await user.click(await screen.findByRole('button', { name: 'Select' }));
    const table = screen.getByRole('treegrid');
    expect(within(table).getAllByRole('checkbox')).toHaveLength(2);
    await user.click(within(table).getByRole('checkbox', { name: 'Select One' }));
    await user.click(within(table).getByRole('checkbox', { name: 'Select Two' }));
    expect(screen.getByText('2 selected')).toBeVisible();
    await user.click(within(screen.getByRole('region', { name: i18n.t('everything.selection') })).getByRole('button', { name: 'Share' }));
    expect(shareMany).toHaveBeenCalledWith([{ kind: 'document', id: 'one', name: 'One' }, { kind: 'topic', id: 'two', name: 'Two' }]);
    expect(namesRead).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Name' }));
    expect(screen.getByText('0 selected')).toBeVisible();
  });

  it('shows a valid fallback without replacing an unavailable saved view until the reader chooses one', async () => {
    const saved = JSON.stringify({ arrangement: 'a_removed_view', sort: 'a_removed_column' });
    writeViewChoice(reader.id, 'everything', 'view', saved, 'mine:all');
    const user = userEvent.setup();
    show(<EverythingBrowser />);
    await screen.findByRole('treegrid');
    expect(screen.getByRole('button', { name: 'Tree' })).toHaveAttribute('aria-pressed', 'true');
    expect(readViewChoice(reader.id, 'everything', 'view', '', 'mine:all')).toBe(saved);
    await user.click(screen.getByRole('button', { name: 'Recent' }));
    expect(JSON.parse(readViewChoice(reader.id, 'everything', 'view', '', 'mine:all'))).toMatchObject({ arrangement: 'recent', sort: 'opened' });
  });

  it('refreshes the exact currently visible levels while a folded cached branch stays quiet', async () => {
    const user = userEvent.setup();
    show(<EverythingBrowser />);
    await user.click(await screen.findByRole('button', { name: 'Expand Acme' }));
    await user.click(await screen.findByRole('button', { name: 'Expand Web · Acme' }));
    await screen.findByRole('link', { name: 'Release plan · Acme / Web' });
    await user.click(screen.getByRole('button', { name: 'Collapse Web · Acme' }));
    const refresh = await screen.findByRole('button', { name: 'Refresh' });
    const before = reads.length;
    await user.click(refresh);
    await waitFor(() => expect(reads.slice(before).map((read) => read.parent ?? 'root').sort()).toEqual(['acme', 'root']));
    expect(screen.queryByRole('link', { name: 'Release plan · Acme / Web' })).not.toBeInTheDocument();
  });
});
