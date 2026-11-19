/** Resource lists keep rich columns, owners and admin marks while the shared query pages and filters first. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Server } from 'lucide-react';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type { EverythingFilter } from '@/api/everything';
import { ResourceList } from '@/features/resources/ResourceList';
import { resourceReaders } from '@/features/resources/inventory';
import type { EverythingRow, Host } from '@/generated/api-types';
import { inventoryRow } from '../doubles/everything-row';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const reader = me(1, 'reader', 'Reader');
const originalSession=client.session.getState();
const owner = userRef(me(2, 'dora', 'Dora'));
const reach = { place: { id: 'org', kind: 'org' as const, name: 'Research', redacted: false }, private: false };
const requests: { parent: string | null; filter: EverythingFilter; cursor: string | null }[] = [];
let failNext = false;

function host(id: string, name: string, native = false): Host {
  return { id, name, address: `team.${id}`, owner, org_id: null, home: null,
    machine: { id: `machine-${id}`, name, online: true, platform_os: 'linux', placement: 'managed' },
    labels: [], drivers: [{ name: 'docker', title: 'Docker', creates: ['environment'], runtime_classes: [] }],
    ports: null, ports_revision: 0, ports_proposal: null, capacity: { cpu_millis: 4000, memory_mb: 8192, disk_gb: 100 },
    declared: { cpu_millis: 1000, memory_mb: 2048, disk_gb: 10 }, schedulable: true,
    when_full: { value: 'warn', own: false }, storages: [], running: { environment: 1, provider: 2 }, mine: 0, revision: 1,
    access: { permissions: ['host.view'], administers: native, participating: !native, admin_only: native ? ['host.view'] : [],
      admin_access: native ? reach : null, through: [] } };
}
const first = host('first', 'Build host');
const later = host('later', 'Later host', true);
const row = (value: Host): EverythingRow => inventoryRow({ id: value.id, kind: 'host', name: value.name, parent_id: 'resources',
  owner: value.owner, participating: value.access.participating, admin_access: value.access.admin_access,
  resource: { kind: 'host', value } });

vi.mock('@/api/auth', () => ({ meQuery: { queryKey: ['resource-reader'], initialData: () => reader, queryFn: () => Promise.resolve(reader) } }));
vi.mock('@/api/everything', () => ({ everythingQuery: (person: string, parent: string | null, filter: EverythingFilter) => ({
  queryKey: ['everything', person, parent, filter], initialPageParam: null as string | null,
  queryFn: ({ pageParam }: { pageParam: string | null }) => {
    requests.push({ parent, filter, cursor: pageParam });
    if (pageParam !== null && failNext) { failNext = false; throw new Error('The next page is unavailable.'); }
    // These are served pages, including a match which was never in the first browser page.
    if (filter.q !== '') return Promise.resolve({ items: [row(later)], next_cursor: null });
    if (filter.participating === true) return Promise.resolve({ items: [row(first)], next_cursor: null });
    return Promise.resolve(pageParam === null ? { items: [row(first)], next_cursor: 'next' } : { items: [row(later)], next_cursor: null });
  },
  getNextPageParam: (page: { next_cursor: string | null }) => page.next_cursor,
}) }));
vi.mock('@/features/resources/trail', () => ({ rememberList: vi.fn(), useResourceHeading: vi.fn() }));
vi.mock('@/features/resources/ResourceNav', () => ({ ResourceNav: () => null }));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabel: () => (place: { name: string | null }) => place.name }));

function List() {
  const [text, setText] = useState('');
  return <ResourceList kind="hosts" search={{}} title="Hosts" icon={Server} lead="Machines that run work" label="Hosts"
    readRow={resourceReaders.hosts} filter={{ state: 'open' }} columns={[
      { id: 'name', header: 'Name', cell: (value) => <a href={`#${value.id}`}>{value.name}</a> },
      { id: 'drivers', header: 'Drivers', cell: (value) => value.drivers.map((driver) => driver.title).join(', ') },
      { id: 'running', header: 'Running', cell: (value) => `${value.running.environment} environments · ${value.running.provider} providers` },
    ]} rowKey={(value) => value.id} text={text} onText={setText} searchLabel="Search hosts" searchPlaceholder="Search hosts"
    empty={{ icon: <Server />, title: 'No hosts' }} noMatch="No matching hosts" />;
}

beforeEach(() => { requests.length = 0; failNext = false; localStorage.clear();client.session.setState({userId:reader.id}); });
afterEach(()=>{client.session.setState(originalSession);localStorage.clear();});

it('loads one page on demand and preserves rich columns, the owner and native-only mark', async () => {
  const user = userEvent.setup();
  show(<List />);
  expect(await screen.findByRole('link', { name: 'Build host' })).toBeVisible();
  expect(screen.queryByRole('link', { name: 'Later host' })).not.toBeInTheDocument();
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({ parent: 'resources', cursor: null, filter: { kinds: ['host'], resource: { state: 'open', details: true } } });
  expect(within(screen.getByRole('table')).getByText('Docker')).toBeVisible();
  expect(screen.getByText('1 environments · 2 providers')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Show more' }));
  expect(await screen.findByRole('link', { name: 'Later host' })).toBeVisible();
  expect(requests.at(-1)?.cursor).toBe('next');
  expect(screen.getByRole('img', { name: 'You see this as an admin of Research' })).toBeVisible();
  for (const item of screen.getAllByRole('row').slice(1)) expect(item.querySelector('[data-resource-owner]')).toHaveTextContent('Dora');
  expect(screen.queryByRole('button', { name: 'Show more' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Filter' }));
  await user.click(screen.getByRole('switch', { name: 'Everything I can see' }));
  expect(await screen.findByRole('link', { name: 'Build host' })).toBeVisible();
  expect(screen.queryByRole('link', { name: 'Later host' })).not.toBeInTheDocument();
  expect(requests.at(-1)?.filter.participating).toBe(true);
});

it('sends focus and host load/running order through that same query and resets the cursor', async () => {
  const user = userEvent.setup();
  show(<List />);
  await screen.findByRole('link', { name: 'Build host' });
  await user.click(screen.getByRole('button', { name: 'Sort' }));
  await user.click(screen.getByRole('combobox', { name: 'Sort by' }));
  await user.click(screen.getByRole('option', { name: 'Load' }));
  await waitFor(() => expect(requests.at(-1)).toMatchObject({ cursor: null, filter: { sort: 'load' } }));
  await user.click(screen.getByRole('combobox', { name: 'Direction' }));
  await user.click(screen.getByRole('option', { name: 'Descending' }));
  await waitFor(() => expect(requests.at(-1)?.filter.direction).toBe('desc'));
  await user.click(screen.getByRole('combobox', { name: 'Sort by' }));
  await user.click(screen.getByRole('option', { name: 'Running' }));
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('button', { name: 'Filter' }));
  await user.click(screen.getByRole('switch', { name: 'Everything I can see' }));
  await waitFor(() => expect(requests.at(-1)).toMatchObject({ cursor: null, filter: { sort: 'running', participating: true } }));
  expect(requests.every((request) => request.parent === 'resources' && request.filter.kinds.length === 1)).toBe(true);
});

it('asks the server for a match outside the loaded page instead of filtering the first page in the browser', async () => {
  const user = userEvent.setup();
  show(<List />);
  await screen.findByRole('link', { name: 'Build host' });
  await user.type(screen.getByRole('searchbox', { name: 'Search hosts' }), 'Later');
  expect(await screen.findByRole('link', { name: 'Later host' })).toBeVisible();
  expect(screen.queryByRole('link', { name: 'Build host' })).not.toBeInTheDocument();
  expect(requests.at(-1)).toMatchObject({ cursor: null, filter: { q: 'Later', resource: { state: 'open', details: true } } });
});

it('keeps loaded rows on a continuation failure and lets the same pager retry', async () => {
  const user = userEvent.setup();
  show(<List />);
  await screen.findByRole('link', { name: 'Build host' });
  failNext = true;
  await user.click(screen.getByRole('button', { name: 'Show more' }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Build host' })).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('link', { name: 'Later host' })).toBeVisible();
});
