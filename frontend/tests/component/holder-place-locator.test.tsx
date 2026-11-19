/** One source-place control keeps the actual native query, current scope and explicit recovery boundary. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { ApiError } from '@/api/client';
import { queryClient as applicationQueries } from '@/api/query';
import { useCandidates, type CandidateOptions } from '@/features/access/candidates';
import type { HolderSearchReply, Org } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { TYPING_PAUSE_MS } from '@/lib/debounce';
import { NetworkError } from '@/lib/requests';
import { me, org, userRef } from '../doubles/fixtures';
import { show } from './support';

interface ReadOptions { params?: { path?: { id?: string }; query?: Record<string, unknown> }; signal?: AbortSignal }
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options?: ReadOptions) => Promise<{ data: unknown }>>(), submit: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: async (path: string, options?: ReadOptions) => ({ ...await calls.read(path, options), response: new Response(null, { status: 200 }) }) } };
});

const HOME = '00000000-0000-4000-8000-000000000001', TOPIC = '00000000-0000-4000-8000-000000000002';
const OTHER = '00000000-0000-4000-8000-000000000003', OTHER_TOPIC = '00000000-0000-4000-8000-000000000004';
const FAR = '00000000-0000-4000-8000-000000000101';
const root = (id = HOME) => org(id, id === HOME ? 'acme' : 'other', id === HOME ? 'Acme' : 'Other', userRef(me(1, 'ada', 'Ada')));
function sourceNode(id: string): Org {
  if (id === HOME || id === OTHER) return root(id);
  const index = listed.findIndex((item) => item.input.kind === 'role' && item.input.at === id);
  if (id !== FAR && index < 0) throw new Error(`Unexpected node ${id}`);
  const parent = root(id === FAR ? OTHER : HOME);
  const name = id === FAR ? 'Remote research' : `Department ${index + 1}`;
  const slug = id === FAR ? 'remote' : `department${index + 1}`;
  return { ...parent, id, name, slug, address: `${parent.address}.${slug}`, parent_id: parent.id, depth: 1,
    path: [...parent.path, { id, kind: 'org', name, redacted: false }] };
}
function row(id: string, path: string, redacted = false): HolderSearchReply['items'][number] {
  return { input: { kind: 'role', role: 'viewer', at: id },
    holder: { kind: 'role', role: 'viewer', at: { id, kind: id === TOPIC || id === OTHER_TOPIC ? 'topic' : 'org', name: redacted ? null : path, redacted }, place: path, count: 0 },
    locator: path.replaceAll(' / ', '.'), place: path, outside: id === FAR };
}
const listed = Array.from({ length: 24 }, (_, index) => row(`00000000-0000-4000-8001-${String(index + 1).padStart(12, '0')}`, `Acme / Department ${index + 1}`));
// This source is in another tree, not a hidden extra page of the complete home-root browse.
const remote = row(FAR, 'Other / Remote research');
const listReads = () => calls.read.mock.calls.filter(([path, options]) => path === '/holders/search' && options?.params?.query?.list_places === true);
const holderReads = () => calls.read.mock.calls.filter(([path, options]) => path === '/holders/search' && options?.params?.query?.list_places === false);
const sourceReads = (q: string) => listReads().filter(([, options]) => options?.params?.query?.q === q);
const denied = () => new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403,
  title: 'Private diagnostic', detail: 'Private diagnostic', request_id: 'source-place' }, null);
interface PickerProps {
  enabled?: boolean;
  browsePlaces?: boolean;
  orgId?: string | null;
  placeId?: string;
  thing?: string;
  kinds?: CandidateOptions['kinds'];
  query?: string;
}
function Picker({ enabled = true, browsePlaces = true, orgId = HOME, placeId = TOPIC, thing = 'Repair login', kinds = ['group', 'role'], query = '' }: PickerProps) {
  const found = useCandidates(query, orgId, { kinds, placeId, thing, enabled, browsePlaces });
  return <form onSubmit={(event) => { event.preventDefault(); calls.submit(); }}>
    {found.placePicker}
    <output aria-label="Holder sources">{[...found.byValue.values()].map((one) => one.input.kind === 'role' ? one.input.at : '').join(',')}</output>
    <ul aria-label="Holder candidates">{[...found.byValue].map(([key, one]) => <li key={key}>{one.name}</li>)}</ul>
    <button type="submit">Commit permission</button>
  </form>;
}
const clients: QueryClient[] = [];
function draw(props: PickerProps = {}) {
  // Keep application freshness/focus/retry semantics. A zero test retry delay
  // exposes a missing source-query retry:false without a one-second sleep.
  const client = new QueryClient({ defaultOptions: { queries: { ...applicationQueries.getDefaultOptions().queries, retryDelay: 0 }, mutations: { retry: false } } });
  clients.push(client);
  const tree = (next: PickerProps) => <QueryClientProvider client={client}><Picker {...next} /></QueryClientProvider>;
  const view = show(tree(props));
  return { client, change: (next: PickerProps) => view.rerender(tree(next)) };
}
const field = () => screen.getByRole('combobox', { name: i18n.t('access.candidates.place') });
const current = () => i18n.t('access.candidates.thisPlace', { place: 'Repair login' });

beforeEach(() => {
  calls.read.mockReset(); calls.submit.mockReset();
  calls.read.mockImplementation((path, options) => {
    if (path === '/nodes/{id}') {
      const id = options?.params?.path?.id;
      if (id === undefined) throw new Error('A native node identifier is required');
      return Promise.resolve({ data: sourceNode(id) });
    }
    if (path !== '/holders/search') throw new Error(`Unexpected request ${path}`);
    const query = options?.params?.query;
    const items = query?.list_places === true
      ? query.q === '' ? query.place_id === OTHER ? [remote] : listed : query.q === 'remote' ? [remote] : []
      : [row(String(query?.place_id), `Holders of ${String(query?.place_id)}`)];
    return Promise.resolve({ data: { items } });
  });
});
afterEach(() => { cleanup(); for (const client of clients.splice(0)) client.clear(); vi.restoreAllMocks(); });

it('browses more than ten places in one field and searches another tree natively without changing holder scope until selection', async () => {
  draw(); const user = userEvent.setup();
  expect(screen.getAllByRole('combobox')).toHaveLength(1);
  expect(field()).toHaveValue(current());
  await waitFor(() => expect(holderReads()).toHaveLength(1)); expect(listReads()).toHaveLength(0);
  await user.click(field()); await screen.findByRole('option', { name: 'Acme / Department 24' });
  expect(screen.getAllByRole('option')).toHaveLength(25);
  expect(listReads()).toHaveLength(1);
  expect(listReads()[0]?.[1]?.params?.query).toEqual({ q: '', place_id: HOME, org_id: HOME, include_work: false, list_places: true });
  await user.click(screen.getByRole('option', { name: 'Acme / Department 24' }));
  const last = listed[23]; if (last?.input.kind !== 'role' || typeof last.input.at !== 'string') throw new Error('The late browse choice needs its exact source ID');
  const lateId = last.input.at;
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toHaveTextContent(lateId));
  expect(holderReads().at(-1)?.[1]?.params?.query?.place_id).toBe(lateId);
  expect(field()).toHaveValue('Acme / Department 24');
  await user.click(field()); await user.type(field(), 'remote');
  await screen.findByRole('option', { name: remote.place });
  expect(listReads().at(-1)?.[1]?.params?.query).toEqual({ q: 'remote', org_id: HOME, include_work: false, list_places: true });
  expect(holderReads()).toHaveLength(2); expect(calls.submit).not.toHaveBeenCalled();
  await user.keyboard('{Enter}');
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toHaveTextContent(FAR));
  expect(holderReads().at(-1)?.[1]?.params?.query).toEqual({ q: '', place_id: FAR, org_id: HOME, include_work: false, list_places: false });
  expect(field()).toHaveValue(remote.place);
  expect(calls.read.mock.calls.some(([path, options]) => path === '/nodes/{id}' && options?.params?.path?.id === FAR)).toBe(false);
  await user.click(field());
  await waitFor(() => expect(listReads().at(-1)?.[1]?.params?.query).toEqual({ q: '', place_id: OTHER, org_id: OTHER, include_work: false, list_places: true }));
  await user.click(screen.getByRole('option', { name: current() }));
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toHaveTextContent(TOPIC));
  expect(field()).toHaveValue(current());
  // Returning to Current reuses its fresh exact query rather than manufacturing another read.
  expect(holderReads()).toHaveLength(3);
  expect(calls.submit).not.toHaveBeenCalled();
});

it('does not submit or choose old rows on one-character, empty-result, dismissed or composing Enter', async () => {
  draw(); const user = userEvent.setup();
  await user.click(field()); await screen.findByRole('option', { name: 'Acme / Department 24' });
  const before = listReads().length;
  await user.type(field(), 'x');
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, TYPING_PAUSE_MS * 2)); });
  expect(screen.queryAllByRole('option')).toHaveLength(0); expect(listReads()).toHaveLength(before);
  await user.keyboard('{Enter}'); expect(calls.submit).not.toHaveBeenCalled();
  await user.clear(field()); await user.type(field(), 'missing');
  expect(await screen.findByText(i18n.t('access.candidates.placeNone'))).toBeVisible();
  await user.keyboard('{Enter}{Escape}{Enter}'); expect(calls.submit).not.toHaveBeenCalled();
  expect(field()).toHaveValue(current());
  await user.click(field()); await user.type(field(), 'remote'); await screen.findByRole('option', { name: remote.place });
  fireEvent.keyDown(field(), { key: 'Enter', code: 'Enter', isComposing: true, keyCode: 229 });
  expect(holderReads()).toHaveLength(1); expect(screen.getByLabelText('Holder sources')).toHaveTextContent(TOPIC);
  expect(calls.submit).not.toHaveBeenCalled();
});

it('isolates late results and discards the query/focus session when the parent closes', async () => {
  let release: ((value: { data: HolderSearchReply }) => void) | undefined;
  const original = calls.read.getMockImplementation(); if (original === undefined) throw new Error('The native reader is required');
  calls.read.mockImplementation((path, options) => options?.params?.query?.q === 'remote'
    ? new Promise((resolve) => { release = resolve; }) : original(path, options));
  const drawn = draw({ enabled: false }); expect(screen.queryByRole('combobox')).toBeNull(); expect(calls.read).not.toHaveBeenCalled();
  drawn.change({}); const user = userEvent.setup(); await user.click(field()); await user.type(field(), 'remote');
  await waitFor(() => expect(release).toBeDefined());
  await user.clear(field()); await user.type(field(), 'missing');
  await waitFor(() => expect(sourceReads('missing')).toHaveLength(1));
  await act(async () => { release?.({ data: { items: [remote] } }); await Promise.resolve(); });
  expect(screen.queryByRole('option', { name: remote.place })).toBeNull();
  expect(sourceReads('remote')).toHaveLength(1); expect(sourceReads('')).toHaveLength(1);
  await user.keyboard('{Enter}'); expect(holderReads()).toHaveLength(1);
  drawn.change({ enabled: false }); const readCount = calls.read.mock.calls.length;
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, TYPING_PAUSE_MS * 2)); });
  expect(calls.read).toHaveBeenCalledTimes(readCount);
  drawn.change({}); expect(field()).toHaveValue(current()); expect(field()).toHaveAttribute('aria-expanded', 'false');
  expect(listReads()).toHaveLength(3); expect(calls.submit).not.toHaveBeenCalled();
});

it('waits for the selected source root and retargets without an old selection or root response', async () => {
  let release: ((value: { data: unknown }) => void) | undefined;
  const original = calls.read.getMockImplementation(); if (original === undefined) throw new Error('The native reader is required');
  calls.read.mockImplementation((path, options) => path === '/nodes/{id}' && options?.params?.path?.id === FAR
    ? new Promise((resolve) => { release = resolve; }) : original(path, options));
  const drawn = draw(); const user = userEvent.setup();
  await user.click(field()); await user.type(field(), 'remote'); await user.click(await screen.findByRole('option', { name: remote.place }));
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toHaveTextContent(FAR));
  const before = listReads().length;
  await user.click(field()); await waitFor(() => expect(release).toBeDefined());
  expect(listReads()).toHaveLength(before); expect(screen.queryByRole('option', { name: 'Acme / Department 1' })).toBeNull();
  drawn.change({ orgId: OTHER, placeId: OTHER_TOPIC, thing: 'Another target' });
  expect(field()).toHaveValue(i18n.t('access.candidates.thisPlace', { place: 'Another target' }));
  expect(field()).toHaveAttribute('aria-expanded', 'false');
  await act(async () => { release?.({ data: sourceNode(FAR) }); await Promise.resolve(); });
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toHaveTextContent(OTHER_TOPIC));
  expect(listReads()).toHaveLength(before);
  expect(holderReads().at(-1)?.[1]?.params?.query?.place_id).toBe(OTHER_TOPIC);
  drawn.change({}); expect(field()).toHaveValue(current());
  expect(field()).toHaveAttribute('aria-expanded', 'false');
});

it('omits redacted names and removes cached place/holder candidates after native permission failures', async () => {
  let refusePlaces = false, refuseHolders = false;
  const original = calls.read.getMockImplementation(); if (original === undefined) throw new Error('The native reader is required');
  calls.read.mockImplementation((path, options) => {
    const query = options?.params?.query;
    if (path === '/holders/search' && (query?.list_places === true ? refusePlaces : refuseHolders)) return Promise.reject(denied());
    if (path === '/holders/search' && query?.list_places === true && query.q === '' && query.place_id === HOME)
      return Promise.resolve({ data: { items: [...listed, row('00000000-0000-4000-8000-000000000999', 'Hidden ancestor / Hidden leaf', true)] } });
    return original(path, options);
  });
  const drawn = draw(); const user = userEvent.setup();
  await user.click(field()); await screen.findByRole('option', { name: 'Acme / Department 24' });
  expect(screen.getAllByRole('option')).toHaveLength(25); expect(screen.queryByText(/Hidden ancestor|Hidden leaf/)).toBeNull();
  refusePlaces = true;
  await act(async () => { await drawn.client.refetchQueries({ queryKey: ['holder-search', '', HOME, HOME, false, true], exact: true }); });
  await screen.findByRole('alert');
  expect(screen.queryByRole('option', { name: 'Acme / Department 24' })).toBeNull();
  expect(screen.getAllByRole('option')).toHaveLength(1); // Only the caller's Current sentinel is left.
  expect(screen.queryByText('Private diagnostic')).toBeNull();
  refuseHolders = true;
  await act(async () => { await drawn.client.refetchQueries({ queryKey: ['holder-search', '', TOPIC, HOME, false, false], exact: true }); });
  expect(drawn.client.getQueryState(['holder-search', '', TOPIC, HOME, false, false])?.status).toBe('error');
  // Query notification is batched after its fetch promise settles.
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toBeEmptyDOMElement());
  expect(screen.getByRole('list', { name: 'Holder candidates' })).toBeEmptyDOMElement();
  expect(calls.submit).not.toHaveBeenCalled();
});

it('keeps a denied selected source opaque and does not browse the original home as a fallback', async () => {
  const original = calls.read.getMockImplementation(); if (original === undefined) throw new Error('The native reader is required');
  calls.read.mockImplementation((path, options) => path === '/nodes/{id}' && options?.params?.path?.id === FAR ? Promise.reject(denied()) : original(path, options));
  draw(); const user = userEvent.setup();
  await user.click(field()); await user.type(field(), 'remote'); await user.click(await screen.findByRole('option', { name: remote.place }));
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toHaveTextContent(FAR));
  const before = listReads().length;
  await user.click(field()); await screen.findByRole('alert');
  expect(listReads()).toHaveLength(before);
  expect(screen.queryByRole('option', { name: remote.place })).toBeNull();
  expect(screen.getByRole('option', { name: i18n.t('access.place.hidden') })).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByLabelText('Holder sources')).toBeEmptyDOMElement();
  expect(screen.getByRole('list', { name: 'Holder candidates' })).toBeEmptyDOMElement();
  await user.keyboard('{Escape}'); expect(field()).toHaveValue(i18n.t('access.place.hidden'));
  expect(holderReads()).toHaveLength(2); expect(calls.submit).not.toHaveBeenCalled();
});

it.each(['root', 'search'] as const)('resets to Current without waiting for a pending %s read and ignores its late answer', async (pending) => {
  let release: ((value: { data: unknown }) => void) | undefined;
  let nodeAttempts = 0;
  const original = calls.read.getMockImplementation(); if (original === undefined) throw new Error('The native reader is required');
  calls.read.mockImplementation((path, options) => {
    if (pending === 'root' && path === '/nodes/{id}' && options?.params?.path?.id === FAR) {
      nodeAttempts += 1;
      // A pending recheck must not clear an earlier denial and redisclose cached holders.
      return nodeAttempts === 1 ? Promise.reject(denied()) : new Promise((resolve) => { release = resolve; });
    }
    if (pending === 'search' && path === '/holders/search' && options?.params?.query?.q === 'waiting')
      return new Promise((resolve) => { release = resolve; });
    return original(path, options);
  });
  draw(); const user = userEvent.setup();
  await user.click(field()); await user.type(field(), 'remote');
  await user.click(await screen.findByRole('option', { name: remote.place }));
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toHaveTextContent(FAR));
  await user.click(field());
  if (pending === 'root') {
    await screen.findByRole('alert');
    expect(screen.getByLabelText('Holder sources')).toBeEmptyDOMElement();
    await user.keyboard('{Escape}'); await user.click(field());
  } else await user.type(field(), 'waiting');
  await waitFor(() => expect(release).toBeDefined());
  expect(screen.getAllByRole('combobox')).toHaveLength(1);
  if (pending === 'root') {
    expect(screen.getByRole('option', { name: current() })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByLabelText('Holder sources')).toBeEmptyDOMElement();
    expect(screen.queryByRole('option', { name: remote.place })).toBeNull();
  } else expect(screen.queryByRole('option')).toBeNull();
  const reset = screen.getByRole('button', { name: current() });
  expect(reset).toBeEnabled();
  const reads = listReads().length;
  if (pending === 'root') { await user.tab(); expect(reset).toHaveFocus(); await user.keyboard('{Enter}'); }
  else await user.click(reset);
  // These assertions run while the source response is still unresolved.
  await waitFor(() => expect(field()).toHaveValue(current()));
  expect(field()).toHaveAttribute('aria-expanded', 'false');
  await waitFor(() => expect(screen.getByLabelText('Holder sources')).toHaveTextContent(TOPIC));
  expect(holderReads()).toHaveLength(2); expect(listReads()).toHaveLength(reads);
  expect(calls.submit).not.toHaveBeenCalled();
  await act(async () => {
    release?.({ data: pending === 'root' ? sourceNode(FAR) : { items: [remote] } });
    await new Promise((resolve) => setTimeout(resolve, TYPING_PAUSE_MS * 2));
  });
  expect(field()).toHaveValue(current());
  expect(field()).toHaveAttribute('aria-expanded', 'false');
  expect(screen.getByLabelText('Holder sources')).toHaveTextContent(TOPIC);
  expect(holderReads()).toHaveLength(2); expect(listReads()).toHaveLength(reads);
  await user.click(field()); await screen.findByRole('option', { name: 'Acme / Department 24' });
  expect(screen.getAllByRole('option')).toHaveLength(25);
  expect(screen.queryByRole('option', { name: remote.place })).toBeNull();
  expect(listReads()).toHaveLength(reads); expect(calls.submit).not.toHaveBeenCalled();
});

it.each(['pointer', 'keyboard'] as const)('keeps Retry reachable after input blur and performs exactly one explicit retry (%s)', async (method) => {
  let release: ((value: { data: HolderSearchReply }) => void) | undefined;
  const original = calls.read.getMockImplementation(); if (original === undefined) throw new Error('The native reader is required');
  let attempts = 0;
  calls.read.mockImplementation((path, options) => {
    if (path === '/holders/search' && options?.params?.query?.q === 'remote') {
      attempts += 1;
      return attempts === 1 ? Promise.reject(new NetworkError('failed')) : new Promise((resolve) => { release = resolve; });
    }
    return original(path, options);
  });
  draw(); const user = userEvent.setup();
  await user.click(field()); await user.type(field(), 'remote');
  await screen.findByRole('alert'); expect(screen.queryByText('Private diagnostic')).toBeNull();
  const retry = screen.getByRole('button', { name: i18n.t('common.tryAgain') });
  expect(sourceReads('remote')).toHaveLength(1);
  if (method === 'pointer') await user.click(retry);
  else {
    await user.tab(); expect(screen.getByRole('button', { name: current() })).toHaveFocus();
    await user.tab(); expect(retry).toHaveFocus(); await user.keyboard('{Enter}');
  }
  await waitFor(() => expect(release).toBeDefined());
  expect(field()).toHaveAttribute('aria-expanded', 'false');
  expect(sourceReads('remote')).toHaveLength(2); expect(holderReads()).toHaveLength(1);
  await act(async () => { release?.({ data: { items: [remote] } }); await Promise.resolve(); });
  await user.click(field());
  await screen.findByRole('option', { name: remote.place });
  expect(sourceReads('remote')).toHaveLength(2); // Reopening reuses the recovered exact query.
  expect(sourceReads('')).toHaveLength(1);
  expect(calls.submit).not.toHaveBeenCalled();
});

it('keeps ordinary user-only and hidden-source consumers on their original native holder query', async () => {
  const drawn = draw({ kinds: ['user'], query: 'Ada', orgId: HOME });
  await waitFor(() => expect(holderReads()).toHaveLength(1));
  expect(screen.queryByRole('combobox')).toBeNull(); expect(listReads()).toHaveLength(0);
  expect(holderReads()[0]?.[1]?.params?.query).toEqual({ q: 'Ada', place_id: TOPIC, org_id: HOME, include_work: false, list_places: false });
  drawn.change({ browsePlaces: false, kinds: ['group', 'role', 'node'] });
  await waitFor(() => expect(holderReads()).toHaveLength(2));
  expect(holderReads().at(-1)?.[1]?.params?.query).toEqual({ q: '', place_id: TOPIC, org_id: HOME, include_work: true, list_places: false });
  expect(screen.queryByRole('combobox')).toBeNull(); expect(listReads()).toHaveLength(0);
});
