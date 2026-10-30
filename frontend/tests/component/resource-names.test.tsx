/** MCP and Mounts lists share primary add controls and explain empty and unmatched lists in either language. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as ClientApi from '@/api/client';
import { ResourceNames } from '@/features/resources/ResourceNames';
import type { EverythingRow, HiddenItem, McpServer, Mount } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { matches } from '@/features/finder/ranking';
import { inventoryRow } from '../doubles/everything-row';
import { me } from '../doubles/fixtures';

import { show } from './support';

const { read, navigate, openAccess } = vi.hoisted(() => ({
  read: vi.fn<(path: string, options: { params: { query?: { q?: string }; path?: { id: string } } }) => Promise<{ data: { items: EverythingRow[]; next_cursor: null } | McpServer | Mount; response: Response }>>(),
  navigate: vi.fn(),
  openAccess: vi.fn(),
}));
let served: (HiddenItem | McpServer | Mount)[] = [];
function inventory(value: HiddenItem | McpServer | Mount): EverythingRow {
  if ('content_hidden' in value) return inventoryRow({ id: value.id, kind: value.kind === 'mount' ? 'mount' : 'mcp_server', name: value.name, my_access: 'path_only' });
  return inventoryRow({ id: value.id, kind: 'target' in value ? 'mount' : 'mcp_server', name: value.name,
    resource: 'target' in value ? { kind: 'mount', value } : { kind: 'mcp_server', value }, owner: value.owner });
}

vi.mock('@/api/auth', () => ({ meQuery: { queryKey: ['resource-names-reader'], initialData: () => me(1, 'reader', 'Reader'), queryFn: () => Promise.resolve(me(1, 'reader', 'Reader')) } }));

vi.mock('@/api/client', async (original) => ({
  ...await original<typeof ClientApi>(),
  api: { GET: read },
}));
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigate,
  useRouter: () => ({ state: { location: { pathname: '/resources/mounts' } }, history: { push: navigate } }),
  Link: ({ to, params, children, ...attributes }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; params?: { id: string } }) => <a {...attributes} href={to.replace('$id', params?.id ?? '')}>{children}</a>,
}));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: vi.fn(), rememberList: vi.fn() }));
vi.mock('@/features/resources/ResourceNav', () => ({ ResourceNav: () => <nav aria-label="Resources" /> }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess }));
vi.mock('@/features/resources/mcp/Edit', () => ({
  EditMcp: () => <div role="dialog" aria-label="MCP editor" />,
}));
vi.mock('@/features/resources/mounts/Edit', () => ({
  EditMount: () => <div role="dialog" aria-label="Mount editor" />,
}));

beforeEach(() => {
  vi.clearAllMocks();
  served = [];
  read.mockImplementation((path, options) => {
    if (path === '/mounts/{id}' || path === '/mcp-servers/{id}') {
      const row = served.find((row) => row.id === options.params.path?.id);
      if (row === undefined || 'content_hidden' in row) throw new Error('Only a readable resource has card details');
      return Promise.resolve({ data: row, response: new Response(null, { status: 200 }) });
    }
    return Promise.resolve({ data: { items: served.map(inventory).filter((row) => matches(options.params.query?.q ?? '', row.name)), next_cursor: null }, response: new Response(null, { status: 200 }) });
  });
});

it('shows every readable mount location in the Where column, linked to its machine or host', async () => {
  vi.spyOn(Element.prototype, 'clientWidth', 'get').mockReturnValue(900);
  const person = { id: 'person', username: 'owner', display_name: 'Owner', kind: 'human' as const, avatar: { color: 1, text: 'O' } };
  const location = { kind: 'path' as const, storage: null, source: null, data_kind: null, can_remove: false, created_by: person, created_at: '2026-10-15T03:30:00Z' };
  const mount: Mount = { id: 'data', name: 'data', address: 'team.data', description: 'Shared data', owner: person, org_id: 'org', home: null,
    target: '/mnt/data', read_only: true, enabled: true, default_fate: 'as_is', kept: [], revision: 1, access: { permissions: ['mount.view'], administers: false, participating: true, admin_only: [], through: [] }, locations: [
      { ...location, machine: { id: 'laptop', name: 'Laptop', online: true, platform_os: 'linux', placement: 'connected' }, host: null },
      { ...location, machine: { id: 'node', name: 'Node', online: true, platform_os: 'linux', placement: 'connected' }, host: { id: 'host', name: 'Build host', address: 'team.host' } },
    ] };
  served = [mount];
  show(<ResourceNames kind="mounts" />);
  const table = await screen.findByRole('table');
  expect(within(table).getByRole('columnheader', { name: 'Where' })).toBeVisible();
  expect(within(table).getByRole('link', { name: 'Laptop' })).toHaveAttribute('href', '/resources/machines/laptop');
  expect(within(table).getByRole('link', { name: 'Build host' })).toHaveAttribute('href', '/resources/hosts/host');
  expect(within(table).queryByText('/srv/private')).not.toBeInTheDocument();
});
afterEach(async () => { vi.restoreAllMocks(); await setLocale('en'); });

function readable(kind: 'mcp' | 'mounts'): McpServer | Mount {
  const base = { id: 'readable', name: 'Readable data', address: 'team.readable', description: 'Ordinary readable content',
    owner: null, org_id: null, home: null, enabled: true, revision: 1,
    access: { permissions: [kind === 'mcp' ? 'mcp.view' : 'mount.view'], administers: false, participating: true, admin_only: [], through: [] } };
  return kind === 'mcp'
    ? { ...base, machine: null, environment: null, imported: null, spec: { transport: 'stdio', command: ['mcp-example'] } }
    : { ...base, target: '/mnt/data', read_only: true, default_fate: 'as_is', locations: [], locations_hidden: 0, kept: [] };
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps mount names and Edit usable beside a compact Where value (${locale}/${theme})`, async () => {
  const appearance = useAppearance.getState(), wasDark = document.documentElement.classList.contains('dark'), previousTheme = document.documentElement.dataset.theme;
  try {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    vi.spyOn(Element.prototype, 'clientWidth', 'get').mockReturnValue(900);
    const source = readable('mounts'); if (!('target' in source)) throw new Error('This case needs a mount');
    const row: Mount = { ...source, name: 'shared-notes-for-team-research', description: 'World 2: shared research notes used across all team environments', access: { ...source.access, permissions: ['mount.view', 'mount.manage'] } };
    served = [row]; const user = userEvent.setup(); show(<ResourceNames kind="mounts" />);
    const table = await screen.findByRole('table'); const name = within(table).getByRole('link', { name: row.name });
    expect(name).toHaveAttribute('href', '/resources/mounts/readable');
    await user.hover(name);
    const card = await screen.findByLabelText(row.name, { selector: '[data-inspection-card]' });
    expect(await within(card).findByRole('link', { name: row.name })).toHaveAttribute('href', '/resources/mounts/readable');
    await user.unhover(name);
    const where = within(table).getByText(i18n.t('resourceDetails.mount.noLocationsShort'), { exact: true });
    await user.click(where); await user.tab({ shift: true }); await user.tab();
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('resourceDetails.mount.noLocationsYet'));
    expect(within(table).queryByText(i18n.t('resourceDetails.mount.noLocationsYet'), { exact: true })).not.toBeInTheDocument();
    await user.click(within(table).getByRole('button', { name: i18n.t('resources.act.of', { act: i18n.t('resources.act.editButton'), name: row.name }) }));
    expect(screen.getByRole('dialog', { name: 'Mount editor' })).toBeVisible();
    expect(read.mock.calls.map(([path]) => path)).toEqual(['/everything', '/mounts/{id}']);
  } finally {
    useAppearance.setState(appearance); document.documentElement.classList.toggle('dark', wasDark); if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme;
  }
});

const CASES = [
  { kind: 'mcp', language: 'en', add: 'Add MCP server', search: 'Search MCP servers',
    noMatch: 'No MCP servers match your search' },
  { kind: 'mounts', language: 'en', add: 'Add mount', search: 'Search mounts',
    noMatch: 'No mounts match your search' },
  { kind: 'mcp', language: 'zh-CN', add: '添加 MCP 服务器', search: '搜索 MCP 服务器',
    noMatch: '没有匹配的 MCP 服务器' },
  { kind: 'mounts', language: 'zh-CN', add: '添加挂载', search: '搜索挂载',
    noMatch: '没有匹配的挂载' },
] as const;

for (const words of CASES) describe(`QS-033 ${words.kind} (${words.language})`, () => {
  it('keeps the permitted edit action visible and opens the editor from its row', async () => {
    await setLocale(words.language);
    const row = readable(words.kind);
    row.access.permissions.push(words.kind === 'mcp' ? 'mcp.manage' : 'mount.manage');
    served = [row]; show(<ResourceNames kind={words.kind} />);
    const table = await screen.findByRole('table');
    const edit = within(table).getByRole('button', { name: i18n.t('resources.act.of', { act: i18n.t('resources.act.editButton'), name: row.name }) });
    expect(edit).toBeVisible(); await userEvent.click(edit);
    expect(screen.getByRole('dialog', { name: words.kind === 'mcp' ? 'MCP editor' : 'Mount editor' })).toBeVisible();
  });

  it('names the empty list, offers one primary add action with a sign and opens its editor by keyboard', async () => {
    await setLocale(words.language);
    const user = userEvent.setup();
    show(<ResourceNames kind={words.kind} />);

    expect(await screen.findByText(i18n.t('library.empty.usable'))).toBeVisible();
    expect(screen.getByRole('button',{name:i18n.t('library.widen.viewable')})).toBeVisible();
    expect(screen.getByRole('combobox',{name:i18n.t('library.show')})).toHaveTextContent(i18n.t('library.views.usable'));
    const buttons = screen.getAllByRole('button', { name: words.add });
    expect(buttons).toHaveLength(1);
    for (const button of buttons) {
      expect(button).toBeVisible();
      expect(button).toHaveClass('bg-accent-solid');
      expect(button.querySelector('svg[aria-hidden="true"]')).not.toBeNull();
    }
    screen.getByRole('searchbox', { name: words.search }).focus();
    await user.tab();
    expect(screen.getByRole('button', { name: i18n.t('resources.controls.filter') })).toHaveFocus();
    await user.tab();
    const order = screen.getByRole('button', { name: i18n.t('resources.controls.sort') });
    expect(order).toHaveFocus();
    await user.keyboard('{Enter}');
    expect(screen.getByRole('combobox', { name: i18n.t('everything.sortBy') })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('combobox', { name: i18n.t('everything.direction') })).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(order).toHaveFocus();
    // Creation lives in the page header, before search and filters in keyboard order.
    buttons[0]?.focus();
    expect(buttons).toContain(document.activeElement);
    await user.keyboard('{Enter}');
    expect(screen.getByRole('dialog', { name: words.kind === 'mcp' ? 'MCP editor' : 'Mount editor' })).toBeVisible();
  });

  it('keeps retired hidden references out of readable rows and distinguishes no matches from an empty list', async () => {
    await setLocale(words.language);
    const hidden: HiddenItem = {
      id: 'private-resource', parent_id: 'org', kind: words.kind === 'mcp' ? 'mcp_server' : 'mount',
      name: 'Private data', owner: null, content_hidden: true, private: true,
    };
    const shown = readable(words.kind);
    served = [shown, hidden];
    const user = userEvent.setup();
    show(<ResourceNames kind={words.kind} />);

    const table = await screen.findByRole('table');
    expect(within(table).getByRole('link', { name: shown.name })).toBeVisible();
    expect(within(table).getByRole('checkbox', { name: i18n.t('resources.bulk.select', { name: shown.name }) })).toBeVisible();
    const open = within(table).getByRole('button', { name: i18n.t('resources.act.of', { act: i18n.t('resources.act.open'), name: shown.name }) });
    expect(open).toBeVisible();
    await user.click(open);
    expect(navigate).toHaveBeenCalledWith({ to: words.kind === 'mcp' ? '/resources/mcp/$id' : '/resources/mounts/$id', params: { id: shown.id } });
    expect(screen.queryByText(i18n.t('library.empty.usable'))).not.toBeInTheDocument();
    expect(screen.queryByText(hidden.name)).not.toBeInTheDocument();
    expect(openAccess).not.toHaveBeenCalled();
    await user.type(screen.getByRole('searchbox', { name: words.search }), 'unmatched');
    expect(await screen.findByText(words.noMatch)).toBeVisible();
    expect(screen.queryByRole('link', { name: shown.name })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: hidden.name })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('library.empty.usable'))).not.toBeInTheDocument();
    expect(openAccess).not.toHaveBeenCalled();
  });

  it('shows the empty readable list when only a legacy hidden reference was returned', async () => {
    await setLocale(words.language);
    const hidden: HiddenItem = { id: 'private-resource', parent_id: 'org', kind: words.kind === 'mcp' ? 'mcp_server' : 'mount',
      name: 'Private data', owner: null, content_hidden: true, private: true };
    served = [hidden];
    show(<ResourceNames kind={words.kind} />);
    expect(await screen.findByText(i18n.t('library.empty.usable'))).toBeVisible();
    expect(screen.queryByText(hidden.name)).not.toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    expect(openAccess).not.toHaveBeenCalled();
  });
});
