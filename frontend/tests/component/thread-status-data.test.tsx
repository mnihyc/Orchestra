/** Status observes shown views' facts; drawing chrome cannot open hidden data readers. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as FilesDoor from '@/features/files/door';
import type * as Usage from '@/api/usage';
import type * as Workspace from '@/api/workspace';
import type * as Terminals from '@/api/terminals';
import { ThreadStatusParts } from '@/features/workbench/StatusParts';
import { useWorkbench } from '@/features/workbench/store';
import { makeGroup } from '@/features/workbench/layout';
import { contentThreadRead } from '@/realtime/threadScope';
import type { Thread } from '@/generated/api-types';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn((): Promise<never> => Promise.reject(new Error('A closed panel must not read'))) }));
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

afterEach(() => { threadWriter.evict(ACME.mainThread); threadWriter.evict(read); useWorkbench.setState(previousWorkbench); calls.read.mockClear(); vi.useRealTimers(); });

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
    expect(screen.getByText(environment.name)).toBeVisible();
    expect(screen.queryByText('Wrong default workspace')).toBeNull();
    await act(async () => { await cache.invalidateQueries(); });
    expect(calls.read).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(120_000); });
    expect(calls.read).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); }
});
