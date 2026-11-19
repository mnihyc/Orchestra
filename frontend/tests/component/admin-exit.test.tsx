/** Only confirmed native mode loss moves an inaccessible page, retaining the frame while its parent is checked. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { meKey } from '@/api/auth';
import { adminModeKey } from '@/api/adminMode';
import { client, type Act } from '@/api/client';
import type * as ClientModule from '@/api/client';
import { nodeKey, orgsKey } from '@/api/nodes';
import { useAdminExit } from '@/shell/useAdminExit';
import { ScreenLoading } from '@/shell/ScreenLoading';
import { i18n, setLocale } from '@/i18n';
import { me, org, personalHomeOf, userRef } from '../doubles/fixtures';

const world = vi.hoisted(() => ({ href: '/project/private', navigate: vi.fn(), read: vi.fn<(path: string, options: unknown) => Promise<{ response: { ok: boolean } }>>(), acts: new Set<(act: Act) => void | Promise<void>>() }));
vi.mock('@tanstack/react-router', () => {
  const router = { get state() { return { location: { href: world.href } }; }, navigate: world.navigate };
  return { useRouter: () => router, useRouterState: () => world.href };
});
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: (path: string, options: unknown) => world.read(path, options) },
    client: { ...actual.client, onAct: (listener: (act: Act) => void | Promise<void>) => { world.acts.add(listener); return () => { world.acts.delete(listener); }; } } };
});
function Frame() { const pending = useAdminExit(); return <><nav aria-label="Frame">Account</nav><main>{pending ? <ScreenLoading /> : <p>Current page</p>}</main></>; }
const caches: QueryClient[] = [];
function setup() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache);
  const person = me(1, 'ada', 'Ada');
  const root = { id: 'root', kind: 'org', name: 'Acme', redacted: false };
  client.session.setState({ userId: person.id });
  cache.setQueryData(meKey, person);
  cache.setQueryData(orgsKey, [org(personalHomeOf(person), 'ada', 'Ada', userRef(person), { org_kind: 'personal' })]);
  cache.setQueryData(adminModeKey, { as_admin: true });
  cache.setQueryData(nodeKey('private'), { id: 'private', kind: 'project', name: 'Private', path: [root], access: { permissions: ['project.view'], admin_access: { place: 'system' } } });
  return cache;
}
async function confirmedOff() { await act(async () => { for (const listener of world.acts) await listener({ method: 'PUT', path: '/me/admin-mode', answer: { as_admin: false } }); }); }
afterEach(async () => { for (const cache of caches.splice(0)) cache.clear(); client.session.setState({ userId: null }); world.href = '/project/private'; vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`pauses the private body and checks its nearest parent after the confirmed Off answer in ${locale}`, async () => {
  await setLocale(locale);
  const cache = setup();
  let answer: (() => void) | undefined;
  world.read.mockImplementation(() => new Promise((resolve) => { answer = () => resolve({ response: { ok: true } }); }));
  render(<QueryClientProvider client={cache}><Frame /></QueryClientProvider>);
  expect(world.navigate).not.toHaveBeenCalled(); // A refused write supplies no act and leaves this frame alone.
  await confirmedOff();
  expect(screen.getByRole('navigation', { name: 'Frame' })).toBeVisible();
  expect(screen.getByRole('status', { name: i18n.t('common.loading') })).toBeVisible();
  expect(screen.queryByText('Current page')).not.toBeInTheDocument();
  expect(world.read).toHaveBeenCalledExactlyOnceWith('/nodes/{id}', expect.objectContaining({ params: { path: { id: 'root' } } }));
  await act(async () => { answer?.(); await Promise.resolve(); });
  await waitFor(() => expect(world.navigate).toHaveBeenCalledWith({ href: '/org/root', replace: true, ignoreBlocker: true }));
});

it('handles a network-reconciled or live Off value without a second write', async () => {
  const cache = setup();
  cache.setQueryData(nodeKey('root'), { id: 'root', access: { permissions: ['org.view'] } });
  render(<QueryClientProvider client={cache}><Frame /></QueryClientProvider>);
  act(() => { cache.setQueryData(adminModeKey, { as_admin: false }); });
  await waitFor(() => expect(world.navigate).toHaveBeenCalledWith({ href: '/org/root', replace: true, ignoreBlocker: true }));
  expect(world.read).not.toHaveBeenCalled();
});

it('keeps an ordinary View page in place when only its administrative actions needed mode', async () => {
  const cache = setup();
  cache.setQueryData(nodeKey('private'), { id: 'private', access: { permissions: ['project.view', 'project.manage'], admin_only: ['project.manage'] } });
  render(<QueryClientProvider client={cache}><Frame /></QueryClientProvider>);
  await confirmedOff();
  expect(world.navigate).not.toHaveBeenCalled();
  expect(world.read).not.toHaveBeenCalled();
  expect(screen.getByText('Current page')).toBeVisible();
});

it('does not redirect a different page when the reader navigated during the parent read', async () => {
  const cache = setup();
  let answer: (() => void) | undefined;
  world.read.mockImplementation(() => new Promise((resolve) => { answer = () => resolve({ response: { ok: true } }); }));
  const view = render(<QueryClientProvider client={cache}><Frame /></QueryClientProvider>);
  await confirmedOff();
  world.href = '/settings/profile';
  view.rerender(<QueryClientProvider client={cache}><Frame /></QueryClientProvider>);
  await act(async () => { answer?.(); await Promise.resolve(); });
  expect(world.navigate).not.toHaveBeenCalled();
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
});
