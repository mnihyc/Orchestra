/** The actual shell opens Find before its single bootstrap answers; pending screens never own the binding. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StrictMode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientModule from '@/api/client';
import { client } from '@/api/client';
import { meStateKey } from '@/api/personal';
import { closeFinder, useFinder } from '@/features/finder/store';
import { useHelpState } from '@/features/help/state';
import { i18n, setLocale } from '@/i18n';
import { SHIPPED_CHOICES } from '@/shell/keyActions';
import { setKeyChoices } from '@/shell/keys';
import { Shell } from '@/shell/Shell';
import { ShellControls } from '@/shell/ShellControls';
import { ScreenLoading } from '@/shell/ScreenLoading';
import { show } from './support';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string) => Promise<unknown>>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: (request: Promise<unknown>) => request };
});
const session = client.session.getState();
const finder = useFinder.getState();
const help = useHelpState.getState();
const caches: QueryClient[] = [];
beforeEach(() => {
  calls.get.mockReset();
  calls.get.mockImplementation((path) => {
    if (path === '/me/bootstrap') return new Promise<unknown>(() => { /* Held until this mounted test shell is retired. */ });
    if (path === '/search/names') return Promise.resolve({ items: [], partial: false });
    if (path === '/settings/registry') return Promise.resolve([]);
    if (path === '/permissions') return Promise.resolve({ permissions: [], roles: [] });
    return Promise.reject(new Error(`Unexpected startup read: ${path}`));
  });
  closeFinder(); useHelpState.setState({ overlay: false }); setKeyChoices(SHIPPED_CHOICES);
  client.session.setState({ status: 'signed_in', userId: 'loading-reader' });
});
afterEach(async () => {
  cleanup(); for (const cache of caches.splice(0)) cache.clear();
  client.session.setState(session); useFinder.setState(finder); useHelpState.setState(help); setKeyChoices(SHIPPED_CHOICES);
  document.documentElement.removeAttribute('data-theme'); await setLocale('en');
});

function draw(keys?: { single_key: boolean; bindings: { finder: string | null } }, admission?: Promise<void>) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache);
  if (keys !== undefined) cache.setQueryData(meStateKey, { keys });
  const root = createRootRoute({ shellComponent: ShellControls, beforeLoad: () => admission, pendingComponent: ScreenLoading,
    component: () => <Shell><p>Screen body has not loaded</p></Shell> });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ['/'] }), defaultPendingMs: 0, defaultPendingMinMs: 0 });
  return show(<StrictMode><QueryClientProvider client={cache}><RouterProvider router={router} /></QueryClientProvider></StrictMode>);
}

it('opens Find even before route admission and the bootstrap reader can mount', async () => {
  let finish: (() => void) | undefined;
  const admission = new Promise<void>((resolve) => { finish = resolve; });
  try {
    draw(undefined, admission);
    await screen.findByRole('status', { name: i18n.t('common.loading') });
    expect(calls.get).not.toHaveBeenCalled();
    await userEvent.setup().keyboard('{Control>}k{/Control}');
    const dialog = await screen.findByRole('dialog', { name: i18n.t('finder.title') });
    expect(within(dialog).getByRole('combobox', { name: i18n.t('finder.field') })).toHaveFocus();
    expect(calls.get.mock.calls.every(([path]) => path === '/search/names')).toBe(true);
    expect(calls.get.mock.calls.map(([path]) => path)).not.toContain('/me/bootstrap');
  } finally { cleanup(); finish?.(); }
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`opens and focuses Find during bootstrap, keeps typing and toggles only once at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup(); const view = draw();
    // Route admission also has a short-lived skeleton. Assert the current status once Shell's
    // actual held bootstrap has mounted, rather than retaining the retired admission element.
    await waitFor(() => {
      expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/bootstrap']);
      expect(screen.getByRole('status', { name: i18n.t('common.loading') })).toBeVisible();
    });
    expect(screen.queryByText('Screen body has not loaded')).not.toBeInTheDocument();
    expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/bootstrap']);
    const opening = useFinder.getState().opened;
    await user.keyboard('{Control>}k{/Control}');
    const dialog = await screen.findByRole('dialog', { name: i18n.t('finder.title') });
    const field = within(dialog).getByRole('combobox', { name: i18n.t('finder.field') });
    expect(field).toHaveFocus(); expect(useFinder.getState().opened).toBe(opening + 1);
    await user.type(field, 'Acme');
    expect(field).toHaveValue('Acme');
    const reads = calls.get.mock.calls.map(([path]) => path);
    expect(reads).toContain('/search/names');
    expect(reads.filter((path) => path === '/me/bootstrap')).toHaveLength(1);
    expect(reads.every((path) => ['/me/bootstrap', '/search/names', '/settings/registry', '/permissions'].includes(path))).toBe(true);
    await user.keyboard('{Control>}k{/Control}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await user.keyboard('{Control>}k{/Control}');
    expect(await screen.findByRole('dialog', { name: i18n.t('finder.title') })).toBeVisible();
    expect(useFinder.getState().opened).toBe(opening + 2);
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    view.unmount();
    const after = useFinder.getState().opened;
    expect(fireEvent.keyDown(document.body, { key: 'k', code: 'KeyK', ctrlKey: true })).toBe(true);
    expect(useFinder.getState().opened).toBe(after); expect(useFinder.getState().open).toBe(false);
  });
}

it('uses cached personal keys without fetching state while bootstrap is pending', async () => {
  const user = userEvent.setup(); draw({ single_key: false, bindings: { finder: 'Mod+P' } });
  await screen.findByRole('status', { name: i18n.t('common.loading') });
  await user.keyboard('{Control>}k{/Control}');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.keyboard('{Control>}p{/Control}');
  expect(await screen.findByRole('dialog', { name: i18n.t('finder.title') })).toBeVisible();
  const reads = calls.get.mock.calls.map(([path]) => path);
  expect(reads.filter((path) => path === '/me/bootstrap')).toHaveLength(1);
  expect(reads.filter((path) => path !== '/me/bootstrap').every((path) => path === '/search/names')).toBe(true);
});

it('keeps composing keys and an absent session out of the shell binding', async () => {
  draw(); await screen.findByRole('status', { name: i18n.t('common.loading') });
  fireEvent.keyDown(document.body, { key: 'k', code: 'KeyK', ctrlKey: true, isComposing: true });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  act(() => { client.session.setState({ userId: null, status: 'signed_out' }); });
  expect(fireEvent.keyDown(document.body, { key: 'k', code: 'KeyK', ctrlKey: true })).toBe(true);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
