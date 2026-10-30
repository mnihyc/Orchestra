/** A served name search reports its own refusal and retries only the visible lookup; local commands stay usable. */
import { QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as ClientModule from '@/api/client';
import { ApiError } from '@/api/client';
import { LiveQueryClient } from '@/api/query';
import { registerCommands } from '@/features/finder/commands';
import { Finder } from '@/features/finder/Finder';
import { closeFinder, openFinder, useFinder } from '@/features/finder/store';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string) => Promise<unknown>>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: (request: Promise<unknown>) => request };
});

const originalFinder = useFinder.getState();
const originalAppearance = useAppearance.getState();
const caches: LiveQueryClient[] = [];
const registrations: (() => void)[] = [];

function refusal(status: 404 | 500): ApiError {
  const code = status === 404 ? 'not_found' : 'internal';
  return new ApiError({ type: `orch:error:${code}`, title: 'Name search failed', code, status,
    detail: 'The name lookup failed.', request_id: 'names-test' }, null);
}

beforeEach(() => {
  calls.get.mockReset(); closeFinder();
});
afterEach(async () => {
  cleanup(); for (const cache of caches.splice(0)) cache.clear();
  for (const remove of registrations.splice(0)) remove();
  useFinder.setState(originalFinder); useAppearance.setState(originalAppearance);
  document.documentElement.removeAttribute('data-theme'); await setLocale('en');
});

function draw(error: ApiError) {
  let fail = true;
  calls.get.mockImplementation((path) => {
    if (path === '/search/names') return fail ? Promise.reject(error) : Promise.resolve({ items: [], partial: false });
    if (path === '/settings/registry') return Promise.resolve([]);
    if (path === '/permissions') return Promise.resolve({ permissions: [], roles: [] });
    return Promise.reject(new Error(`Unexpected Finder read: ${path}`));
  });
  const run = vi.fn();
  registrations.push(registerCommands([{ id: 'test.local', kind: 'create', title: () => 'Local command', run }]));
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache);
  const background = vi.fn(() => Promise.resolve({ items: [], partial: false }));
  cache.setQueryDefaults(['names', 'historical'], { queryFn: background });
  cache.setQueryData(['names', 'historical'], { items: [], partial: false });
  const root = createRootRoute({ component: () => <Finder ready={false} /> });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ['/'] }) });
  openFinder('local');
  show(<QueryClientProvider client={cache}><RouterProvider router={router} /></QueryClientProvider>);
  return { run, background, recover: () => { fail = false; } };
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`shows a 404 as its actual cause and keeps local commands at ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale }); document.documentElement.dataset.theme = theme;
    const { run, background } = draw(refusal(404));
    const dialog = await screen.findByRole('dialog', { name: i18n.t('finder.title') });
    const alert = await within(dialog).findByRole('alert');
    expect(alert).toHaveTextContent(i18n.t('errors.not_found'));
    expect(within(dialog).queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
    expect(within(dialog).queryByText(i18n.t('finder.nothing', { query: 'local' }))).not.toBeInTheDocument();
    expect(within(dialog).getByRole('combobox', { name: i18n.t('finder.field') })).toHaveFocus();
    const command = within(dialog).getByRole('option', { name: `Local command ${i18n.t('finder.commandKinds.create')}` });
    expect(command).toBeVisible();
    await userEvent.click(command);
    await waitFor(() => expect(run).toHaveBeenCalledExactlyOnceWith('here'));
    expect(background).not.toHaveBeenCalled();
  });

  it(`retries a temporary failure without reading background names at ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale }); document.documentElement.dataset.theme = theme;
    const { background, recover } = draw(refusal(500));
    const user = userEvent.setup();
    const dialog = await screen.findByRole('dialog', { name: i18n.t('finder.title') });
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(i18n.t('errors.internal'));
    const before = calls.get.mock.calls.filter(([path]) => path === '/search/names').length;
    recover();
    await user.click(within(dialog).getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(within(dialog).queryByRole('alert')).not.toBeInTheDocument());
    expect(calls.get.mock.calls.filter(([path]) => path === '/search/names')).toHaveLength(before + 3);
    expect(within(dialog).getByRole('option', { name: `Local command ${i18n.t('finder.commandKinds.create')}` })).toBeVisible();
    expect(background).not.toHaveBeenCalled();
  });
}
