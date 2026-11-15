/** Status observes shown views' facts; drawing chrome cannot open hidden data readers. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useChangeHost } from '@/features/change/target';
import { ChangeHost } from '@/features/change/ChangeHost';
// Compile the lazy plan module before measuring the command's state and request handoff.
import '@/features/change/ThreadChange';
import { openWhere } from '@/features/thread/openWhere';
import type * as Nodes from '@/api/nodes';
import type * as Forks from '@/api/forks';
import { afterEach, expect, it, vi } from 'vitest';
import type * as FilesDoor from '@/features/files/door';
import type * as Usage from '@/api/usage';
import type * as Workspace from '@/api/workspace';
import type * as Terminals from '@/api/terminals';
import { ThreadStatusParts } from '@/features/workbench/StatusParts';
import { useWorkbench } from '@/features/workbench/store';
import { makeGroup } from '@/features/workbench/layout';
import { contentThreadRead } from '@/realtime/threadScope';
import type { CapStatus, Place, Thread } from '@/generated/api-types';
import { placeLimitsQuery } from '@/api/usage';
import { capTitle } from '@/features/usage/words';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { choiceCap } from '../unit/resource-facts-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn((): Promise<never> => Promise.reject(new Error('A closed panel must not read'))),
  places: vi.fn<() => Promise<Place[]>>().mockResolvedValue([]), plan: vi.fn<typeof Forks.planFork>().mockRejectedValue(new Error('The controlled plan was refused')) }));
let native: Thread;
const previousWorkbench = useWorkbench.getState();
const owner = 'actual-status-content';
const read = contentThreadRead(ACME.mainThread, owner);
vi.mock('@/features/files/door', async (load) => ({ ...await load<typeof FilesDoor>(), useRouteThread: () => ACME.mainThread,
  useThreadDoor: () => {
    const environment = native.environment;
    if (environment === null) throw new Error('The status fixture needs its environment');
    return { kind: 'ready', access: { door: { environmentId: environment.id, threadId: native.id }, thread: native, environment,
      directory: native.directory, mayView: true, mayManage: true, mayWatchProcesses: true, mayShell: true } };
  },
}));
vi.mock('@/api/usage', async (load) => {
  const actual = await load<typeof Usage>();
  return { ...actual,
    healthQuery: (...args: Parameters<typeof Usage.healthQuery>) => ({ ...actual.healthQuery(...args), queryFn: calls.read }),
    placeLimitsQuery: (...args: Parameters<typeof Usage.placeLimitsQuery>) => ({ ...actual.placeLimitsQuery(...args), queryFn: calls.read }),
  };
});
vi.mock('@/api/workspace', async (load) => {
  const actual = await load<typeof Workspace>();
  return { ...actual,
    gitStatusQuery: (...args: Parameters<typeof Workspace.gitStatusQuery>) => ({ ...actual.gitStatusQuery(...args), queryFn: calls.read }),
    changesQuery: (...args: Parameters<typeof Workspace.changesQuery>) => ({ ...actual.changesQuery(...args), queryFn: calls.read }),
  };
});
vi.mock('@/api/terminals', async (load) => {
  const actual = await load<typeof Terminals>();
  return { ...actual, shellsQuery: (...args: Parameters<typeof Terminals.shellsQuery>) => ({ ...actual.shellsQuery(...args), queryFn: calls.read }) };
});
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(), nodeQuery: (id: string) => ({
  queryKey: ['location-thread', id], queryFn: () => Promise.resolve(native),
}) }));
vi.mock('@/api/forks', async (load) => {
  const actual = await load<typeof Forks>();
  return { ...actual, planFork: calls.plan, placesQuery: (...args: Parameters<typeof Forks.placesQuery>) => ({ ...actual.placesQuery(...args), queryFn: calls.places }) };
});

afterEach(async () => { threadWriter.evict(ACME.mainThread); threadWriter.evict(read); useWorkbench.setState(previousWorkbench); useChangeHost.setState({ current: null, open: false }); calls.read.mockClear(); calls.places.mockClear(); calls.plan.mockClear(); vi.useRealTimers(); await setLocale('en'); });

function showBlockedStatus(blocked: Thread['blocked'], caps: CapStatus[] = []) {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The status fixture needs its native thread');
  native = { ...world.threads.thread(person, row), state: 'blocked', blocked };
  threadWriter.reset(read, { thread: native }, false);
  useWorkbench.setState({ layout: { ...useWorkbench.getState().layout, main: makeGroup([{ id: owner, view: 'page', of: { href: `/thread/${native.id}` } }]) }, activeContent: owner });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(placeLimitsQuery(native.id).queryKey, { caps });
  return show(<QueryClientProvider client={cache}><ThreadStatusParts /></QueryClientProvider>);
}

for (const locale of ['en', 'zh-CN'] as const) {
  it.each([null, 'The vendor asks us to wait for the weekly reset'])('keeps a provider limit distinct from a cap without cap facts in '+locale+' (%s)', async (detail) => {
    await setLocale(locale);
    showBlockedStatus({ reason: 'provider_exhausted', detail });
    const title = i18n.t('workbench.status.providerLimit');
    expect(screen.getByRole('button', { name: detail === null ? title : `${title} · ${detail}` })).toBeVisible();
    expect(screen.queryByText(i18n.t('workbench.status.limitReached', { name: i18n.t('providerPlan.limits') }), { exact: true })).not.toBeInTheDocument();
    expect(calls.read).not.toHaveBeenCalled();
  });

  it.each(['warning', 'reached'] as const)('uses the measured cap name and %s state when the server supplies it in '+locale, async (state) => {
    await setLocale(locale);
    const cap = choiceCap({ state });
    showBlockedStatus({ reason: 'provider_exhausted', detail: 'Vendor notice' }, [cap]);
    const label = i18n.t(state === 'reached' ? 'workbench.status.limitReached' : 'workbench.status.limitNear', { name: capTitle(cap.cap, i18n.t) });
    expect(screen.getByRole('button', { name: label })).toBeVisible();
    expect(screen.queryByText(i18n.t('workbench.status.providerLimit'), { exact: true })).not.toBeInTheDocument();
    expect(calls.read).not.toHaveBeenCalled();
  });
}

it('renders the native environment without fetching hidden limits, health, Git, changes or terminals, even after invalidation and idle', async () => {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The status fixture needs its native thread');
  native = world.threads.thread(person, row);
  const environment = native.environment;
  if (environment === null) throw new Error('The status fixture needs its native environment');
  threadWriter.reset(native.id, { thread: { ...native, environment: { ...environment, name: 'Wrong default workspace' } } }, false);
  threadWriter.reset(read, { thread: native }, false);
  useWorkbench.setState({ layout: { ...useWorkbench.getState().layout, main: makeGroup([{ id: owner, view: 'page', of: { href: `/thread/${native.id}` } }]) }, activeContent: owner });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.useFakeTimers();
  const view = show(<QueryClientProvider client={cache}><ThreadStatusParts /></QueryClientProvider>);
  try {
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue(environment.name);
    expect(screen.queryByText('Wrong default workspace')).toBeNull();
    await act(async () => { await cache.invalidateQueries(); });
    expect(calls.read).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(120_000); });
    expect(calls.read).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); }
});

it('offers directory editing in the thread bar and carries it to the ordinary switch plan', async () => {
  showBlockedStatus(null);
  const user = userEvent.setup();
  await user.click(screen.getByRole('combobox', { name: i18n.t('start.where.title') }));
  await user.click(screen.getByRole('button', { name: i18n.t('change.directory.change') }));
  const directory = screen.getByRole('textbox', { name: i18n.t('change.directory.label') });
  await user.clear(directory);
  await user.type(directory, 'apps/web');
  await user.click(screen.getByRole('button', { name: i18n.t('start.where.review') }));
  expect(useChangeHost.getState().current).toMatchObject({ target: { kind: 'thread', id: native.id },
    options: { kind: 'switch', focus: 'environment', placement: { directory: 'apps/web' } } });
  useChangeHost.setState({ current: null, open: false });
});

it('opens the same location field through the directory command entrance', async () => {
  const bar = showBlockedStatus(null);
  bar.unmount();
  act(() => openWhere(native));
  show(<ChangeHost />);
  const dialog = await screen.findByRole('dialog', { name: i18n.t('change.directory.label') });
  expect(await within(dialog).findByRole('combobox', { name: i18n.t('start.where.title') })).toBeVisible();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t('change.directory.change') }));
  const directory = screen.getByRole('textbox', { name: i18n.t('change.directory.label') });
  await user.clear(directory);
  await user.type(directory, 'apps/native');
  await user.click(screen.getByRole('button', { name: i18n.t('start.where.review') }));
  expect(useChangeHost.getState().current).toMatchObject({ target: { kind: 'thread', id: native.id },
    options: { kind: 'switch', focus: 'environment', placement: { directory: 'apps/native' } } });
  expect(useChangeHost.getState().open).toBe(true);
  await waitFor(() => expect(calls.plan).toHaveBeenCalled());
  expect(calls.plan.mock.calls.at(-1)?.[1]).toMatchObject({ directory: 'apps/native' });
});
