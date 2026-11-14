/** Binding reads belong to their cache flight and selected environment; pending changes retain loaded facts. */
import { QueryClient, QueryClientProvider, useQuery, useQueryClient } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StrictMode, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { nodeKey } from '@/api/nodes';
import type { CreationAccessContext, StartOptions } from '@/api/start';
import type { Node } from '@/generated/api-types';
import { NO_PICKS, type Picks } from '@/features/start/choices';
import { rememberSetup } from '@/features/start/lastSetup';
import { useStartOptions } from '@/features/start/options';
import { i18n, setLocale } from '@/i18n';
import { agent, environment, options, provider } from '../unit/start-fixtures';
import { deferred } from './deferred';
import { show } from './support';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

interface Request { params: { path: { id: string }; query: { environment_id?: string; environment_new: boolean; template_id?: string; host_id?: string; host_auto?: boolean } }; signal?: AbortSignal }
interface Reply { data?: StartOptions; error?: unknown; response: Response }
const calls = vi.hoisted(() => ({ get: vi.fn<(_path: string, _request: Request) => Promise<Reply>>() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: calls.get } };
});
const success = (data: StartOptions): Reply => ({ data, response: new Response(null, { status: 200 }) });
const refused = (disabled = false): Reply => {
  const status = disabled ? 400 : 403, code = disabled ? 'environment.none_usable' : 'authz.denied';
  return { response: new Response(null, { status }), error: {
    type: `orch:error:${code}`, code, status, title: 'Use needed', detail: 'This environment is unavailable.', request_id: 'binding-refused',
  } };
};
let cache: QueryClient;
function Form({ at = 'topic', known, initial = NO_PICKS, audience }: { at?: string; known?: string; initial?: Picks; audience?: CreationAccessContext }) {
  cache = useQueryClient();
  const [picks, setPicks] = useState(initial);
  const [draft, setDraft] = useState('First message');
  const state = useStartOptions(at, picks, { suggest: true }, true, audience, known);
  return <section>
    <input aria-label="Draft" value={draft} onChange={(event) => setDraft(event.target.value)} />
    <span data-provider>{state.options?.providers[0]?.provider.name ?? ''}</span>
    <span data-environment>{state.resolved?.environment?.environment.name ?? ''}</span>
    <span data-origin>{state.resolved?.origins.environment ?? ''}</span>
    <span data-checking>{state.refreshing ? i18n.t('start.quick.checking') : state.loading ? 'initial' : 'ready'}</span>
    <span data-failed>{state.failed === null ? '' : 'failed'}</span>
    <button onClick={() => setPicks((held) => ({ ...held, environment: 'e2' }))}>Choose e2</button>
    <button onClick={() => setPicks((held) => ({ ...held, model: 'm2', effort: 'low' }))}>Choose model</button>
    <button onClick={() => setPicks((held) => ({ ...held, environment: 'new', newEnvironment: { template_id: 'alternate', host_id: 'quiet' } }))}>Choose template</button>
    <button onClick={() => setPicks((held) => ({ ...held, newEnvironment: { template_id: 'alternate', host_id: null } }))}>Choose scheduler</button>
    <button onClick={state.retry}>Retry</button>
  </section>;
}
const before = client.session.getState();
beforeEach(() => {
  calls.get.mockReset().mockResolvedValue(success(options()));
  client.session.setState({ ...before, userId: 'binding-reader', readerVersion: 1, status: 'signed_in' });
});
afterEach(async () => { cleanup(); client.session.setState(before); await setLocale('en'); });
const requested = () => calls.get.mock.calls.map(([, request]) => request.params.query.environment_id ?? (request.params.query.environment_new ? 'new' : null));
function ParentNode({ node, read }: { node: Node; read: Promise<Node> }) {
  useQuery({ queryKey: nodeKey(node.id), queryFn: () => read });
  return <Form at={node.id} />;
}

it('reads the selected template and host through the real API client and keeps the scheduler in its own cache flight', async () => {
  calls.get.mockImplementation((_path, request) => Promise.resolve(success(request.params.query.template_id === 'alternate'
    ? options({ environment_default: 'fresh', providers: [provider(request.params.query.host_auto === true ? 'p3' : 'p2')] }) : options())));
  const user = userEvent.setup();
  show(<Form known="e1" />);
  await waitFor(() => expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p1'));
  await user.click(screen.getByRole('button', { name: 'Choose template' }));
  await waitFor(() => expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p2'));
  expect(calls.get.mock.calls.at(-1)?.[1].params.query).toMatchObject({ environment_new: true, template_id: 'alternate', host_id: 'quiet', host_auto: false });
  await user.click(screen.getByRole('button', { name: 'Choose scheduler' }));
  await waitFor(() => expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p3'));
  const automatic = calls.get.mock.calls.at(-1)?.[1].params.query;
  expect(automatic).toMatchObject({ environment_new: true, template_id: 'alternate', host_auto: true });
  expect(automatic).not.toHaveProperty('host_id');
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`waits for the pending topic node and sends its default in the first binding read in ${locale}`, async () => {
    await setLocale(locale);
    const parent = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    const pending = deferred<Node>();
    const world = new World(), person = world.people.get('ada');
    if (person === undefined) throw new Error('The topic metadata needs its native reader');
    const node = world.places.node(person, ACME.fixLogin);
    if (node.kind !== 'topic') throw new Error('The starter needs its native topic');
    const view = show(<QueryClientProvider client={parent}><StrictMode><ParentNode node={node} read={pending.promise} /></StrictMode></QueryClientProvider>);
    try {
      await userEvent.type(screen.getByRole('textbox', { name: 'Draft' }), ' while metadata loads');
      expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveValue('First message while metadata loads');
      expect(requested()).toEqual([]);
      await act(async () => { pending.resolve({ ...node, default_environment_id: 'e1' }); await Promise.resolve(); });
      await waitFor(() => expect(requested()).toEqual(['e1']));
      await waitFor(() => expect(document.querySelector('[data-environment]')).toHaveTextContent('Env e1'));
      await userEvent.click(screen.getByRole('button', { name: 'Choose model' }));
      expect(requested()).toEqual(['e1']);
    } finally { view.unmount(); parent.clear(); }
  });

  it(`uses cached topic metadata without another node read in ${locale}`, async () => {
    await setLocale(locale);
    const parent = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    const world = new World(), person = world.people.get('ada');
    if (person === undefined) throw new Error('The topic metadata needs its native reader');
    const node = world.places.node(person, ACME.fixLogin);
    if (node.kind !== 'topic') throw new Error('The starter needs its native topic');
    parent.setQueryData(nodeKey(node.id), { ...node, default_environment_id: 'e1' });
    const view = show(<QueryClientProvider client={parent}><Form at={node.id} /></QueryClientProvider>);
    try {
      await waitFor(() => expect(document.querySelector('[data-environment]')).toHaveTextContent('Env e1'));
      expect(requested()).toEqual(['e1']);
      expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/nodes/{id}/binding/options']);
    } finally { view.unmount(); parent.clear(); }
  });

  it(`judges an explicit environment while topic metadata is pending in ${locale}`, async () => {
    await setLocale(locale);
    const parent = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    const pending = deferred<Node>();
    const metadata = parent.query({ queryKey: nodeKey(ACME.fixLogin), queryFn: () => pending.promise });
    const world = new World(), person = world.people.get('ada');
    if (person === undefined) throw new Error('The topic metadata needs its native reader');
    const node = world.places.node(person, ACME.fixLogin);
    if (node.kind !== 'topic') throw new Error('The starter needs its native topic');
    const view = show(<QueryClientProvider client={parent}><Form at={node.id} /></QueryClientProvider>);
    try {
      expect(requested()).toEqual([]);
      await userEvent.click(screen.getByRole('button', { name: 'Choose e2' }));
      await waitFor(() => expect(requested()).toEqual(['e2']));
      await waitFor(() => expect(document.querySelector('[data-environment]')).toHaveTextContent('Env e2'));
      await act(async () => { pending.resolve({ ...node, default_environment_id: 'e1' }); await metadata; });
      expect(requested()).toEqual(['e2']);
      expect(document.querySelector('[data-environment]')).toHaveTextContent('Env e2');
    } finally { view.unmount(); parent.clear(); }
  });

  it(`starts one cache-owned read with the known environment across Strict Mode remounts in ${locale}`, async () => {
    await setLocale(locale);
    const pending = deferred<StartOptions>(); calls.get.mockImplementation(() => pending.promise.then(success));
    show(<StrictMode><Form known="e1" /></StrictMode>);
    await waitFor(() => expect(calls.get).toHaveBeenCalledTimes(1));
    expect(requested()).toEqual(['e1']);
    expect(calls.get.mock.calls[0]?.[1]).not.toHaveProperty('signal');
    await act(async () => { pending.resolve(options()); await Promise.resolve(); });
    await waitFor(() => expect(document.querySelector('[data-environment]')).toHaveTextContent('Env e1'));
    await userEvent.click(screen.getByRole('button', { name: 'Choose model' }));
    expect(requested()).toEqual(['e1']);
  });

  it(`retains loaded facts and draft while checking another environment, and retries only it in ${locale}`, async () => {
    await setLocale(locale);
    const pending = deferred<StartOptions>();
    calls.get.mockImplementation((_path, request) => request.params.query.environment_id === 'e2' ? pending.promise.then(success) : Promise.resolve(success(options())));
    show(<Form known="e1" />);
    await waitFor(() => expect(document.querySelector('[data-environment]')).toHaveTextContent('Env e1'));
    await userEvent.type(screen.getByRole('textbox', { name: 'Draft' }), ' retained');
    await userEvent.click(screen.getByRole('button', { name: 'Choose e2' }));
    await waitFor(() => expect(requested()).toEqual(['e1', 'e2']));
    expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p1');
    expect(document.querySelector('[data-checking]')).toHaveTextContent(i18n.t('start.quick.checking'));
    expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveValue('First message retained');
    await userEvent.click(screen.getByRole('button', { name: 'Choose model' }));
    expect(requested()).toEqual(['e1', 'e2']);
    await act(async () => { pending.resolve(options()); await Promise.resolve(); });
    await waitFor(() => expect(document.querySelector('[data-checking]')).toHaveTextContent('ready'));
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(requested()).toEqual(['e1', 'e2', 'e2']));
  });

  it(`uses a remembered environment in its first request and preserves its source in ${locale}`, async () => {
    await setLocale(locale);
    rememberSetup('binding-reader', { place: 'topic', picks: { ...NO_PICKS, environment: 'e2' } });
    calls.get.mockResolvedValue(success(options({ environments: [environment('e1'), environment('e2')] })));
    show(<Form />);
    await waitFor(() => expect(document.querySelector('[data-environment]')).toHaveTextContent('Env e2'));
    expect(requested()).toEqual(['e2']);
    expect(document.querySelector('[data-origin]')).toHaveTextContent('last_used');
  });

  it(`reads only the newly discovered no-host fallback and keeps its first answer visible in ${locale}`, async () => {
    await setLocale(locale);
    const base = options({ hosts: [], environment_default: 'fresh', providers: [provider('p1', { agents: [agent('a1', { reason: 'no_host' })] })] });
    const pending = deferred<StartOptions>();
    calls.get.mockImplementation((_path, request) => request.params.query.environment_id === undefined ? Promise.resolve(success(base)) : pending.promise.then(success));
    show(<Form />);
    await waitFor(() => expect(requested()).toEqual([null, 'e1']));
    expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p1');
    expect(document.querySelector('[data-checking]')).toHaveTextContent(i18n.t('start.quick.checking'));
    await act(async () => { pending.resolve(options({ hosts: [], environment_default: 'fresh' })); await Promise.resolve(); });
    await waitFor(() => expect(document.querySelector('[data-checking]')).toHaveTextContent('ready'));
    expect(document.querySelector('[data-origin]')).toHaveTextContent('suggested');
    expect(requested()).toEqual([null, 'e1']);
  });

  it.each([false, true])(`rechecks a refused automatic hint but preserves explicit refusals in ${locale} (disabled: %s)`, async (disabled) => {
    await setLocale(locale);
    calls.get.mockImplementation((_path, request) => Promise.resolve(request.params.query.environment_id === 'removed' ? refused(disabled) : success(options({ environment_default: 'fresh' }))));
    const view = show(<Form known="removed" />);
    await waitFor(() => expect(requested()).toEqual(['removed', null]));
    await waitFor(() => expect(document.querySelector('[data-checking]')).toHaveTextContent('ready'));
    expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p1');
    await waitFor(() => expect(document.querySelector('[data-failed]')).toBeEmptyDOMElement());
    view.unmount(); cache.clear(); calls.get.mockClear();
    show(<Form initial={{ ...NO_PICKS, environment: 'removed' }} />);
    await waitFor(() => expect(document.querySelector('[data-failed]')).toHaveTextContent('failed'));
    expect(requested()).toEqual(['removed']);
  });

  it(`never keeps the previous place's form snapshot while another place loads in ${locale}`, async () => {
    await setLocale(locale);
    const pending = deferred<StartOptions>();
    calls.get.mockImplementation((_path, request) => request.params.path.id === 'other' ? pending.promise.then(success) : Promise.resolve(success(options())));
    const view = show(<Form known="e1" />);
    await waitFor(() => expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p1'));
    view.rerender(<Form at="other" known="e1" />);
    expect(document.querySelector('[data-provider]')).toBeEmptyDOMElement();
    await act(async () => { pending.resolve(options()); await Promise.resolve(); });
  });

  it.each(['reader', 'audience'] as const)(`does not borrow a loaded snapshot across %s changes in ${locale}`, async (change) => {
    await setLocale(locale);
    const view = show(<Form known="e1" />);
    await waitFor(() => expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p1'));
    const pending = deferred<StartOptions>(); calls.get.mockImplementation(() => pending.promise.then(success));
    if (change === 'reader') act(() => { cache.clear(); client.session.setState({ userId: 'another-binding-reader', readerVersion: 2 }); });
    else view.rerender(<Form known="e1" audience={{ threadAccess: 'private' }} />);
    expect(document.querySelector('[data-provider]')).toBeEmptyDOMElement();
    await waitFor(() => expect(calls.get).toHaveBeenCalledTimes(2));
    if (change === 'audience') expect(calls.get.mock.calls[1]?.[1]).toMatchObject({ params: { query: { thread_access: JSON.stringify('private') } } });
    await act(async () => { pending.resolve(options({ providers: [provider('p2')] })); await Promise.resolve(); });
    await waitFor(() => expect(document.querySelector('[data-provider]')).toHaveTextContent('Provider p2'));
  });
}
