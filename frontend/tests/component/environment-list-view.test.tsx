import type * as RouterModule from '@tanstack/react-router';
import { resourceCardRead, type CardReadOptions } from './resource-card-readers';
/** The real Environment list starts with usable rows and keeps each deliberate device view. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientApi from '@/api/client';
import { client } from '@/api/client';
import { EnvironmentList } from '@/features/resources/environments/EnvironmentList';
import type { Environment, EverythingRowPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { forgetViewChoiceDeviceData } from '@/lib/viewChoices';
import { GRACE } from '../doubles/chat-rows';
import { resourceInventoryRow as inventoryRow } from '../doubles/everything-row';
import { me } from '../doubles/fixtures';
import { detailEnvironment } from './resource-detail-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

interface ReadOptions extends CardReadOptions { params: { query?: Record<string, unknown>; path?: { id?: string; object?: string } } }
const read = vi.hoisted(() => vi.fn<(path: string, options: ReadOptions) => Promise<{ data: EverythingRowPage; response: Response }>>());
const reader = me(942, 'environment-reader', 'Environment reader');
const original = client.session.getState();
let emptyUsable = false;
function environment(id: string, name: string, usable: boolean): Environment {
  return { ...detailEnvironment, id, name, owner: GRACE, placement: 'connected', host: null,
    access: { permissions: usable ? ['environment.view', 'environment.use'] : ['environment.view'], administers: false, participating: true, admin_only: [], through: [] } };
}
const usable = environment('usable-environment', 'Usable workspace', true);
const viewOnly = environment('view-environment', 'View-only workspace', false);
const inventory = (row: Environment) => inventoryRow({ id: row.id, kind: 'environment', name: row.name, owner: row.owner, parent_id: 'resources', resource: { kind: 'environment', value: row } });

vi.mock('@/api/client', async (load) => ({ ...await load<typeof ClientApi>(), api: { GET: (path: string, options: ReadOptions) => { const auxiliary = resourceCardRead(path, options); return auxiliary === null ? read(path, options) : Promise.resolve(auxiliary); } } }));
vi.mock('@/api/auth', async (loadOriginal) => ({ ...await loadOriginal<object>(), meQuery: { queryKey: ['environment-list-reader'], initialData: () => reader, queryFn: () => Promise.resolve(reader) } }));
vi.mock('@/features/resources/HomePicker', () => ({ useViewedOrgState: () => ({ org: { id: 'workshop', name: 'Workshop', access: { administers: true } }, load: { isSuccess: true, isError: false } }) }));
vi.mock('@/features/resources/trail', () => ({ rememberList: vi.fn(), useResourceHeading: vi.fn() }));
vi.mock('@/features/resources/ResourceNav', () => ({ ResourceNav: () => null }));
vi.mock('@/features/navigation/placeLabels', async (loadOriginal) => ({ ...await loadOriginal<object>(), useKnownPlaces: () => new Map() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/environments/acts', () => ({ useEnvironmentActs: () => ({ list: { name: (row: Environment) => row.name, of: () => ({ primary: null, entries: [], keys: {}, busy: false }), bulk: [] } }) }));
vi.mock('@tanstack/react-router', async (loadRouter) => ({ ...await loadRouter<typeof RouterModule>(), useNavigate: () => vi.fn(), Link: ({ to, params, children, ...attributes }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; params?: { id: string } }) => <a {...attributes} href={to.replace('$id', params?.id ?? '')}>{children}</a> }));

function draw() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return show(<QueryClientProvider client={cache}><EnvironmentList search={{}} onSearch={vi.fn()} /></QueryClientProvider>);
}
beforeEach(async () => {
  await setLocale('en'); localStorage.clear(); forgetViewChoiceDeviceData(reader.id, null); client.session.setState({ userId: reader.id }); emptyUsable = false;
  read.mockReset(); read.mockImplementation((path, options) => {
    if (path !== '/everything') throw new Error('This list must use the shared resource inventory');
    const rows = options.params.query?.resource_view === 'usable' ? emptyUsable ? [] : [usable] : [usable, viewOnly];
    return Promise.resolve({ data: { items: rows.map(inventory), next_cursor: null }, response: new Response(null, { status: 200 }) });
  });
});
afterEach(() => { cleanup(); localStorage.clear(); forgetViewChoiceDeviceData(reader.id, null); client.session.setState(original); });

it('uses the fresh-device Use view, preserves chosen Viewable and All, and widens an empty Use view', async () => {
  const user = userEvent.setup(); let opened = draw();
  await screen.findByRole('link', { name: usable.name });
  const showLabel = i18n.t('library.show');
  let showControl = screen.getByRole('combobox', { name: showLabel });
  expect(showControl).toHaveTextContent(i18n.t('library.views.usable'));
  expect(screen.queryByRole('link', { name: viewOnly.name })).not.toBeInTheDocument();
  expect(read.mock.calls[0]).toMatchObject(['/everything', { params: { query: { kinds: 'environment', parent: 'resources', resource_rows: true, resource_view: 'usable' } } }]);

  await user.click(showControl);
  await user.click(screen.getByRole('option', { name: i18n.t('library.views.viewable') }));
  expect(await screen.findByRole('link', { name: viewOnly.name })).toBeVisible(); opened.unmount();
  opened = draw();
  showControl = screen.getByRole('combobox', { name: showLabel });
  expect(showControl).toHaveTextContent(i18n.t('library.views.viewable'));
  expect(await screen.findByRole('link', { name: viewOnly.name })).toBeVisible();

  await user.click(showControl);
  await user.click(screen.getByRole('option', { name: i18n.t('library.views.all', { org: 'Workshop' }) }));
  expect(await screen.findByRole('link', { name: viewOnly.name })).toBeVisible(); opened.unmount();
  opened = draw();
  showControl = screen.getByRole('combobox', { name: showLabel });
  expect(showControl).toHaveTextContent(i18n.t('library.views.all', { org: 'Workshop' }));
  await screen.findByRole('link', { name: viewOnly.name });
  expect(read.mock.calls.at(-1)?.[1].params.query).toMatchObject({ resource_view: 'all', home: 'workshop' });

  emptyUsable = true;
  await user.click(showControl);
  await user.click(screen.getByRole('option', { name: i18n.t('library.views.usable') }));
  expect(await screen.findByText(i18n.t('library.empty.usable'))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('library.widen.viewable') }));
  expect(await screen.findByRole('link', { name: viewOnly.name })).toBeVisible();
  expect(showControl).toHaveTextContent(i18n.t('library.views.viewable'));
  opened.unmount();
});
