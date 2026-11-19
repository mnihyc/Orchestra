import { resourceCardRead } from './resource-card-readers';
/** Hints keep their ordinary and warning states; card reads stay independent of the number of names on screen. */
import { QueryClient, QueryClientProvider, QueryObserver, useQuery } from '@tanstack/react-query';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import { LiveQueryClient, queryClient } from '@/api/query';
import { providerQuery } from '@/api/resources';
import { applyChanges, resourceChanges, userEventChanges } from '@/api/acts';
import type { Provider } from '@/generated/api-types';
import { socket } from '@/realtime/live';
import type { FeedListener } from '@/realtime/socket';
import { ErrorState } from '@/ui/ErrorState';
import { ResourceHint, ResourceName, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { Choice } from '@/ui/Choice';
import ResourceCard from '@/features/resource-facts/ResourceCard';
import type { ResourceSubject } from '@/features/resource-facts/facts';
import { detailEnvironment, detailHost } from './resource-detail-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { choiceCap } from '../unit/resource-facts-fixtures';
import type { CapStatus } from '@/generated/api-types';
import { show as showFrame } from './support';
import type { ReactNode } from 'react';

function show(ui: ReactNode) {
  const cache = new QueryClient({ defaultOptions: queryClient.getDefaultOptions() });
  return showFrame(<QueryClientProvider client={cache}>{ui}</QueryClientProvider>);
}

const requests = vi.hoisted(() => ({ provider: vi.fn<(id: string) => void>(), caps: [] as CapStatus[], reply: null as null | (() => Promise<Provider>) }));
vi.mock('@/api/client', async (load) => ({ ...await load<object>(), api: { GET: async (path: string, options: { params: { path: { id: string; object?: string } } }) => {
  const id = options.params.path.id;
  const { planProvider } = await import('../unit/provider-plan-fixtures');
  const auxiliary = resourceCardRead(path, options);
  if (auxiliary !== null) return auxiliary;
  if (path !== '/providers/{id}') throw new Error(`Unexpected resource-card read ${path}`);
  requests.provider(id);
  return { data: requests.reply === null ? planProvider({ id, name: `Provider ${id}`, choice_usage: { caps: requests.caps, performance: [], observed_at: '2026-10-25T12:00:00Z' } }) : await requests.reply(), error: undefined, response: new Response(null, { status: 200 }) };
} } }));
beforeEach(() => { requests.provider.mockClear(); requests.caps = []; requests.reply = null; });

const provider = planProvider();
it('clears only the resource stream’s own failure on accepted subscription without another read', async () => {
  const id = 'accepted-provider', options = providerQuery(id), cache = queryClient;
  cache.setQueryData(options.queryKey, planProvider({ id, revision: 6 }));
  let listener: FeedListener | undefined;
  const subscribed = vi.spyOn(socket, 'subscribe').mockImplementation((feed, next) => {
    if (feed === `resource:provider:${id}`) listener = next;
    return { unsubscribe: () => undefined };
  });
  const observer = new QueryObserver(cache, { ...options, staleTime: Infinity }), off = observer.subscribe(() => undefined);
  try {
    await waitFor(() => expect(listener).toBeDefined());
    listener?.onFrame({ op: 'error', id: 1, code: 'too_many_subscriptions', detail: 'capacity' });
    expect(observer.getCurrentResult().isError).toBe(true);
    listener?.onFrame({ op: 'subscribed', id: 2, feed: `resource:provider:${id}`, mode: 'delta', seq: 6 });
    expect(observer.getCurrentResult()).toMatchObject({ isError: false, data: { revision: 6 } });
    const failure = new Error('An independent HTTP read failed');
    cache.getQueryCache().find({ queryKey: options.queryKey, exact: true })?.setState({ error: failure, status: 'error' });
    listener?.onFrame({ op: 'subscribed', id: 2, feed: `resource:provider:${id}`, mode: 'delta', seq: 6 });
    expect(observer.getCurrentResult().error).toBe(failure); expect(requests.provider).not.toHaveBeenCalled();
  } finally { off(); subscribed.mockRestore(); cache.removeQueries({ queryKey: options.queryKey, exact: true }); }
});

it('retires a hidden provider hint and offers exact recovery when its returning live feed is refused', async () => {
  const id = 'hidden-changed-provider', options = providerQuery(id), cache = queryClient;
  cache.setQueryData(options.queryKey, planProvider({ id, revision: 6, name: 'Kept provider' }));
  const listeners = new Set<FeedListener>();
  const subscribed = vi.spyOn(socket, 'subscribe').mockImplementation((feed, listener) => {
    if (feed === `resource:provider:${id}`) listeners.add(listener);
    return { unsubscribe: () => { listeners.delete(listener); } };
  });
  const observer = new QueryObserver(cache, { ...options, staleTime: 60_000 });
  const off = observer.subscribe(() => undefined);
  let finish!: (row: Provider) => void;
  requests.reply = () => new Promise<Provider>((resolve) => { finish = resolve; });
  let view: ReturnType<typeof showFrame> | undefined;
  function Detail() {
    const read = useQuery(options);
    return <><h1>{read.data?.name}</h1>{read.isError && <ErrorState compact error={read.error} onRetry={() => void read.refetch()} />}</>;
  }
  try {
    await waitFor(() => expect(listeners.size).toBe(1)); off();
    await waitFor(() => expect(listeners.size).toBe(0));
    await applyChanges(userEventChanges('resources', { kind: 'provider', id }), cache);
    expect(requests.provider).not.toHaveBeenCalled();
    expect(cache.getQueryState(options.queryKey)?.isInvalidated).toBe(true);
    view = showFrame(<Detail />, cache);
    expect(screen.getByRole('heading', { name: 'Kept provider' })).toBeVisible();
    await waitFor(() => { expect(requests.provider).toHaveBeenCalledOnce(); expect(listeners.size).toBe(1); });
    const listener = [...listeners][0];
    act(() => listener?.onFrame({ op: 'error', id: 1, code: 'too_many_subscriptions', detail: 'capacity' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Live update capacity reached');
    await act(async () => { finish(planProvider({ id, revision: 7, name: 'Earlier read' })); await Promise.resolve(); });
    expect(screen.getByRole('heading', { name: 'Kept provider' })).toBeVisible();
    requests.reply = () => Promise.resolve(planProvider({ id, revision: 8, name: 'Recovered provider' }));
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('heading', { name: 'Recovered provider' })).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument(); expect(requests.provider).toHaveBeenCalledTimes(2);
  } finally { view?.unmount(); off(); subscribed.mockRestore(); cache.removeQueries({ queryKey: options.queryKey, exact: true }); }
});

it.each(['success', 'failure'] as const)('keeps the newer native provider when an older HTTP read ends with %s', async (outcome) => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = providerQuery(provider.id).queryKey;
  let finish!: (value: Provider) => void, refuse!: (error: Error) => void;
  requests.reply = () => new Promise<Provider>((resolve, reject) => { finish = resolve; refuse = reject; });
  cache.setQueryData(key, planProvider({ revision: 6, name: 'Previously read' }));
  const older = cache.query({ ...providerQuery(provider.id), staleTime: 0 });
  try {
    await waitFor(() => expect(requests.provider).toHaveBeenCalledOnce());
    await applyChanges(resourceChanges('provider', provider.id, { row: planProvider({ revision: 8, name: 'Newer native provider' }) }), cache);
    if (outcome === 'success') finish(planProvider({ revision: 7, name: 'Obsolete HTTP provider' }));
    else refuse(new Error('Obsolete transport failed'));
    expect((await older).revision).toBe(8);
    expect(cache.getQueryState(key)).toMatchObject({ data: { revision: 8, name: 'Newer native provider' }, status: 'success', error: null, fetchStatus: 'idle' });
    expect(requests.provider).toHaveBeenCalledOnce();
  } finally { finish(planProvider()); await older.catch(() => undefined); cache.clear(); }
});
it.each([false, true])('wraps the complete provider window description below a picker name with warning=%s', (warning) => {
  const current = planProvider({ facts: { ...provider.facts, windows: provider.facts.windows.map((window) => ({ ...window, warning_percent: warning ? 80 : null })) } });
  const option = resourceChoice({ kind: 'provider', provider: current });
  const view = show(<div style={{ width: 160 }}><Choice name={current.name} description={option.description} presentation="picker" /></div>);
  const hint = view.container.querySelector('[data-choice-description] [data-resource-hint="provider"]');
  expect(hint).toHaveTextContent(/Weekly window.*81%.*resets/);
  expect(hint).toHaveClass('whitespace-normal', 'wrap-anywhere');
  expect(hint).not.toHaveClass('truncate');
  if (warning) expect(hint).toHaveAttribute('data-warning', 'true');
  else expect(hint).not.toHaveAttribute('data-warning');
  expect(requests.provider).not.toHaveBeenCalled();
});
const model = { model_name: 'model', display_name: 'Model', parameters: { context_window: 200_000 } };
it('reads missing model limits once and updates the same card hint from the permitted provider', async () => {
  requests.caps = [choiceCap({ state: 'reached' })];
  show(<ResourceCard subject={{ kind: 'model', model, providerId: 'model-provider' }} />);
  expect(await screen.findByRole('list', { name: 'Limits' })).toBeVisible();
  const hint = document.querySelector('[data-resource-hint="model"]');
  expect(hint).toHaveAttribute('data-warning', 'true');
  expect(hint).toHaveTextContent(/^At limit/);
  expect(requests.provider).toHaveBeenCalledExactlyOnceWith('model-provider');
});
const subjects: { subject: ResourceSubject; warning: boolean; text: RegExp }[] = [
  { subject: { kind: 'provider', provider }, warning: false, text: /81%/ },
  { subject: { kind: 'provider', provider: { ...provider, status: 'offline' } }, warning: true, text: /Unavailable/ },
  { subject: { kind: 'model', model, providerId: provider.id }, warning: false, text: /200K/ },
  { subject: { kind: 'model', model, providerId: provider.id, usage: { caps: [choiceCap({ state: 'reached' })], performance: [], observed_at: '2026-10-25T12:00:00Z' } }, warning: true, text: /At limit/ },
  { subject: { kind: 'host', host: detailHost }, warning: false, text: /running/ },
  { subject: { kind: 'host', host: { ...detailHost, full: true } }, warning: true, text: /^Full$/ },
  { subject: { kind: 'environment', environment: detailEnvironment }, warning: false, text: /Running/ },
  { subject: { kind: 'environment', environment: { ...detailEnvironment, status: 'offline' } }, warning: true, text: /Host offline/ },
];
for (const { subject, warning, text } of subjects) it(`renders a ${warning ? 'warning' : 'quiet'} ${subject.kind} hint from its permitted row`, () => {
  show(<ResourceHint subject={subject} />);
  const hint = screen.getByText(text).closest('[data-resource-hint]');
  expect(hint).not.toBeNull();
  expect(hint).toHaveAttribute('data-resource-hint', subject.kind);
  expect(hint).toHaveClass(warning ? 'text-warning-text' : 'text-text-muted');
  expect(requests.provider).not.toHaveBeenCalled();
});

for (const count of [1, 20]) it(`renders ${count} provider names with zero detail reads and reuses one opened detail`, async () => {
  const user = userEvent.setup();
  const cache = new QueryClient({ defaultOptions: queryClient.getDefaultOptions() });
  show(<QueryClientProvider client={cache}><div>{Array.from({ length: count }, (_, index) => <ResourceName key={index} subject={{ kind: 'provider', provider: { id: String(index), name: `Provider ${index}`, status: 'ready' } }} />)}</div></QueryClientProvider>);
  expect(requests.provider).not.toHaveBeenCalled();
  const first = screen.getByRole('button', { name: 'Provider 0' });
  await user.click(first);
  expect(await screen.findByRole('link', { name: 'Open provider' })).toHaveAttribute('href', '/resources/providers/0');
  expect(requests.provider).toHaveBeenCalledExactlyOnceWith('0');
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('link', { name: 'Open provider' })).not.toBeInTheDocument());
  await user.click(first);
  expect(await screen.findByRole('link', { name: 'Open provider' })).toBeVisible();
  expect(requests.provider).toHaveBeenCalledExactlyOnceWith('0');
});

it('uses supplied list facts without another read and preserves the two windows and resets', async () => {
  const user = userEvent.setup();
  show(<ResourceName subject={{ kind: 'provider', provider }} />);
  await user.click(screen.getByRole('button', { name: provider.name }));
  expect(await screen.findByRole('meter', { name: '5-hour window' })).toBeVisible();
  expect(screen.getByRole('meter', { name: 'Weekly window (Fable)' })).toBeVisible();
  expect(screen.getByText('Reset time not set')).toBeVisible();
  expect(requests.provider).not.toHaveBeenCalled();
});

it('keeps private vendor facts and cap observations out of a resource card', () => {
  const privateProvider = planProvider({ choice_usage: { caps: [choiceCap({ state: 'reached' })], performance: [], observed_at: '2026-10-25T12:00:00Z' } });
  privateProvider.access.permissions = ['provider.use'];
  show(<ResourceCard subject={{ kind: 'provider', provider: privateProvider }} />);
  expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  expect(screen.queryByRole('list', { name: 'Limits' })).not.toBeInTheDocument();
  expect(screen.queryByText('Active turns')).not.toBeInTheDocument();
  expect(requests.provider).not.toHaveBeenCalled();
});
