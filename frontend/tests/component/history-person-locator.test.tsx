/** Actual History/Trace controls and native query factories; only HTTP replies and thread transport are synthetic. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef, useState, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientApi from '@/api/client';
import type * as Live from '@/realtime/live';
import type * as Router from '@tanstack/react-router';
import { ApiError } from '@/api/client';
import { PEOPLE_PAGE } from '@/api/access';
import { wireFilters } from '@/api/trace';
import { HistoryView } from '@/features/inspector/History';
import { PersonLocator } from '@/features/people/PersonLocator';
import { TraceFilters } from '@/features/trace/Filters';
import { traceHref, type TraceSearch, type TraceScope } from '@/features/trace/params';
import { ViewInstanceContext } from '@/features/workbench/instance';
import type { HistoryLine, HistoryLinePage, UserRef } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { TYPING_PAUSE_MS } from '@/lib/debounce';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { resultThreadRead } from '@/realtime/threadScope';
import { threadWriter } from '@/realtime/stores/thread';
import { TooltipProvider } from '@/ui/Tooltip';
import { me, userRef } from '../doubles/fixtures';
import { deferred, snapshot } from '../unit/continuation-fixtures';

interface Params { path?: { object: string; id: string }; query: { actor?: string; depth?: string; cursor?: string; q?: string; org_id?: string; limit?: number } }
interface ReadOptions { params: Params; signal: AbortSignal }
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options: ReadOptions) => Promise<{ data: unknown; response: Response }>>(), writes: vi.fn(), follows: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof ClientApi>();
  return { ...actual, api: { ...actual.api, GET: calls.read, POST: calls.writes, PATCH: calls.writes, PUT: calls.writes, DELETE: calls.writes } };
});
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: (target: unknown) => { calls.follows(target); } }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useParams: () => ({ threadId: 'unrelated-route-thread' }) }));

const picked = userRef(me(71, 'rare.person', 'Rare Person')), replacement = userRef(me(72, 'other.person', 'Other Person'));
const loadedActor = userRef(me(73, 'first.page', 'First Page Person'));
const otherThread = '01950000-0000-7000-8000-000000000074';
const owner = 'owned-history-inspector';
let threadId: string, orgId: string, cache: QueryClient, user: ReturnType<typeof userEvent.setup>;
let history: (params: Params, signal: AbortSignal) => Promise<HistoryLinePage>;
let people: (q: string, org: string | undefined, signal: AbortSignal) => Promise<UserRef[]>;
const row = (text: string, actor: HistoryLine['actor'] = null): HistoryLine => ({ id: text, text, actor, about: null, action: 'thread.created', at: '2026-10-18T00:00:00Z' });
const page = (text: string, next_cursor: string | null = null, actor: HistoryLine['actor'] = null): HistoryLinePage => ({ items: [row(text, actor)], next_cursor });
const failure = (code: 'internal' | 'authz.denied' = 'internal') => new ApiError({ type: `orch:error:${code}`, code,
  title: 'Private native diagnostic', detail: 'Private native diagnostic', status: code === 'internal' ? 500 : 403, request_id: 'private-request' }, null);
const historyCalls = () => calls.read.mock.calls.filter(([path]) => path === '/{object}/{id}/history');
const peopleCalls = () => calls.read.mock.calls.filter(([path]) => path === '/users/search');
const personInput = (label = i18n.t('threadViews.history.person')) => screen.getByRole('combobox', { name: label });

function Frame({ id = threadId }: { id?: string }) {
  const read = resultThreadRead(id, owner);
  return <ViewInstanceContext.Provider value={{ id: owner, root: createRef(), target: { kind: 'thread', id }, headingOwner: owner, content: false, focusedContent: false }}>
    <ThreadReadProvider read={read}><HistoryView tab={{ id: owner, view: 'history' }} area="right" /></ThreadReadProvider>
  </ViewInstanceContext.Provider>;
}
function draw(children: ReactNode) {
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={cache}><TooltipProvider>{children}</TooltipProvider></QueryClientProvider>;
  return render(children, { wrapper: Wrapper });
}
async function search(text: string, label?: string) {
  const input = personInput(label);
  await user.click(input); await user.clear(input); await user.type(input, text);
  return input;
}
async function choose(person = picked, label?: string) {
  await search(person.username, label);
  await user.click(await screen.findByRole('option', { name: person.display_name }));
}

beforeEach(() => {
  cleanup(); threadWriter.clear(); calls.read.mockReset(); calls.writes.mockReset(); calls.follows.mockReset();
  cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } }); user = userEvent.setup();
  const cut = snapshot(); threadId = cut.thread.id; orgId = cut.thread.org_id;
  for (const id of [threadId, otherThread]) threadWriter.reset(resultThreadRead(id, owner), { ...cut, thread: { ...cut.thread, id, environment: null } }, false);
  history = () => Promise.resolve(page('Only the loaded first-page actor', 'unfiltered-next', loadedActor));
  people = () => Promise.resolve([picked]);
  calls.read.mockImplementation(async (path, { params, signal }) => {
    const data = path === '/{object}/{id}/history' ? await history(params, signal)
      : path === '/users/search' ? await people(params.query.q ?? '', params.query.org_id, signal)
        : (() => { throw new Error(`Unexpected native read ${path}`); })();
    return { data, response: new Response(null, { status: 200 }) };
  });
});
afterEach(() => { cleanup(); cache.clear(); threadWriter.clear(); vi.restoreAllMocks(); vi.useRealTimers(); });

it('locates an actor absent from loaded history and sends that UUID before paging at either depth', async () => {
  history = ({ query }) => Promise.resolve(query.actor === picked.id
    ? page(query.depth === 'every' ? 'Every selected-actor change' : query.cursor === undefined ? 'Selected actor first page' : 'Selected actor second page', query.cursor === undefined && query.depth === 'summary' ? 'actor-next' : null)
    : page('Only the loaded first-page actor', 'unfiltered-next', loadedActor));
  draw(<Frame />); await screen.findByText('Only the loaded first-page actor');
  expect(peopleCalls()).toHaveLength(0); expect(historyCalls()).toHaveLength(1);
  await search(picked.username);
  expect(await screen.findByRole('option', { name: picked.display_name })).toBeVisible();
  expect(screen.queryByRole('option', { name: loadedActor.display_name })).toBeNull();
  expect(historyCalls()).toHaveLength(1);
  await user.keyboard('{Enter}');
  await screen.findByText('Selected actor first page');
  expect(personInput()).toHaveValue(picked.display_name); expect(screen.queryByText('Only the loaded first-page actor')).toBeNull();
  expect(peopleCalls()[0]?.[1].params.query).toEqual({ q: picked.username });
  expect(historyCalls().at(-1)?.[1].params).toMatchObject({ path: { object: 'nodes', id: threadId }, query: { depth: 'summary', actor: picked.id } });
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  await screen.findByText('Selected actor second page');
  expect(historyCalls().at(-1)?.[1].params.query).toMatchObject({ actor: picked.id, depth: 'summary', cursor: 'actor-next' });
  const depth = screen.getByRole('radiogroup', { name: i18n.t('places.activity.depth') });
  await user.click(within(depth).getAllByRole('radio')[1]!);
  await screen.findByText('Every selected-actor change');
  expect(historyCalls().at(-1)?.[1].params.query).toEqual({ actor: picked.id, depth: 'every', limit: PEOPLE_PAGE });
  expect(personInput()).toHaveValue(picked.display_name); expect(calls.writes).not.toHaveBeenCalled();
});

it('keeps the selected identity and clear/reselect controls during a pending or failed actor history, and ignores a cleared request’s late page', async () => {
  const pending = deferred<HistoryLinePage>(); let selectedReads = 0;
  history = async ({ query }) => {
    if (query.actor === undefined) return page('Unfiltered history');
    selectedReads += 1;
    if (selectedReads === 1) return pending.promise;
    if (selectedReads === 2) throw failure();
    return page('Actor recovered through manual retry');
  };
  draw(<Frame />); await screen.findByText('Unfiltered history'); await choose();
  await waitFor(() => expect(selectedReads).toBe(1));
  expect(personInput()).toHaveValue(picked.display_name);
  await user.click(screen.getByRole('button', { name: i18n.t('trace.clearPerson') }));
  expect(await screen.findByText('Unfiltered history')).toBeVisible();
  await act(async () => { pending.resolve(page('Late cleared actor page')); await Promise.resolve(); });
  expect(screen.queryByText('Late cleared actor page')).toBeNull();
  await choose(); await screen.findByRole('alert');
  expect(personInput()).toHaveValue(picked.display_name); expect(screen.getByRole('button', { name: i18n.t('trace.clearPerson') })).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await screen.findByText('Actor recovered through manual retry');
  expect(selectedReads).toBe(3);
  expect(historyCalls().filter(([, value]) => value.params.query.actor !== undefined).every(([, value]) => value.params.query.actor === picked.id && value.params.query.cursor === undefined)).toBe(true);
  expect(calls.writes).not.toHaveBeenCalled();
});

it('keeps a committed person while search hits change or fail, and retries only the exact native lookup', async () => {
  const pending = deferred<UserRef[]>(); let attempts = 0;
  people = async () => { attempts += 1; if (attempts === 1) return pending.promise; return [replacement]; };
  const selected = vi.fn();
  function Locator() {
    const [person, setPerson] = useState<UserRef | null>(picked);
    return <PersonLocator label="Person locator" value={person?.id ?? null} person={person} orgId={orgId} onChoose={(next) => { selected(next); setPerson(next); }} />;
  }
  draw(<Locator />); expect(personInput('Person locator')).toHaveValue(picked.display_name); expect(peopleCalls()).toHaveLength(0);
  await search('other', 'Person locator'); await waitFor(() => expect(peopleCalls()).toHaveLength(1));
  expect(screen.queryByRole('option')).toBeNull(); expect(selected).not.toHaveBeenCalled();
  await user.keyboard('{Escape}'); expect(personInput('Person locator')).toHaveValue(picked.display_name);
  await act(async () => { pending.reject(failure()); await Promise.resolve(); });
  await screen.findByRole('alert'); expect(personInput('Person locator')).toHaveValue(picked.display_name);
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(attempts).toBe(2));
  await user.click(personInput('Person locator'));
  await user.click(await screen.findByRole('option', { name: replacement.display_name }));
  expect(selected).toHaveBeenCalledTimes(1); expect(selected).toHaveBeenCalledWith(replacement); expect(personInput('Person locator')).toHaveValue(replacement.display_name);
  expect(peopleCalls().map(([, value]) => value.params.query)).toEqual([{ q: 'other', org_id: orgId }, { q: 'other', org_id: orgId }]);
  expect(calls.writes).not.toHaveBeenCalled();
});

it('does not fetch an empty or one-code-point directory search, including one surrogate-pair character', async () => {
  vi.useFakeTimers(); draw(<PersonLocator label="Person locator" value={null} person={null} onChoose={vi.fn()} />);
  const input = personInput('Person locator');
  fireEvent.focus(input);
  await act(async () => { await vi.advanceTimersByTimeAsync(TYPING_PAUSE_MS); });
  fireEvent.change(input, { target: { value: '😀' } });
  await act(async () => { await vi.advanceTimersByTimeAsync(TYPING_PAUSE_MS); });
  expect(peopleCalls()).toHaveLength(0);
  fireEvent.change(input, { target: { value: '😀a' } });
  await act(async () => { await vi.advanceTimersByTimeAsync(TYPING_PAUSE_MS); });
  expect(peopleCalls()).toHaveLength(1); expect(peopleCalls()[0]?.[1].params.query).toEqual({ q: '😀a' });
  expect(calls.writes).not.toHaveBeenCalled();
});

it('resets actor/search/depth when its actual thread changes and never admits late hits from the previous target or the browser route', async () => {
  const pending = deferred<UserRef[]>();
  people = async (q) => q === 'pending' ? pending.promise : [picked];
  history = ({ path, query }) => Promise.resolve(page(path?.id === otherThread ? 'Other thread native history' : query.actor === undefined ? 'Original thread history' : 'Selected original history'));
  const drawn = draw(<Frame />); await screen.findByText('Original thread history'); await choose(); await screen.findByText('Selected original history');
  await user.click(within(screen.getByRole('radiogroup', { name: i18n.t('places.activity.depth') })).getAllByRole('radio')[1]!);
  await search('pending'); await waitFor(() => expect(peopleCalls().at(-1)?.[1].params.query.q).toBe('pending'));
  drawn.rerender(<Frame id={otherThread} />); await screen.findByText('Other thread native history');
  expect(personInput()).toHaveValue(i18n.t('trace.anyPerson'));
  expect(historyCalls().at(-1)?.[1].params).toMatchObject({ path: { id: otherThread }, query: { depth: 'summary' } });
  expect(historyCalls().at(-1)?.[1].params.query.actor).toBeUndefined();
  await act(async () => { pending.resolve([replacement]); await Promise.resolve(); });
  expect(personInput()).toHaveValue(i18n.t('trace.anyPerson')); expect(screen.queryByRole('option')).toBeNull();
  expect(historyCalls().every(([, value]) => [threadId, otherThread].includes(value.params.path?.id ?? ''))).toBe(true);
  expect(calls.follows.mock.calls.every(([target]) => [threadId, otherThread].includes(target as string))).toBe(true);
});

it('suppresses cached history and the retained person label when the native target is refused', async () => {
  history = ({ query }) => query.cursor !== undefined ? Promise.reject(failure('authz.denied'))
    : Promise.resolve(query.actor === undefined ? page('Unfiltered history') : page('Previously permitted actor history', 'actor-next'));
  draw(<Frame />); await screen.findByText('Unfiltered history'); await choose(); await screen.findByText('Previously permitted actor history');
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  await screen.findByRole('alert');
  expect(screen.queryByText('Previously permitted actor history')).toBeNull();
  expect(screen.queryByRole('combobox', { name: i18n.t('threadViews.history.person') })).toBeNull();
  expect(screen.queryByDisplayValue(picked.display_name)).toBeNull(); expect(screen.queryByText('Private native diagnostic')).toBeNull();
});

it('shares the locator with Trace while preserving its org restriction, include_agents and exact URL/native filters', async () => {
  const scope: TraceScope = { type: 'node', id: threadId, name: 'Owned thread', kind: 'thread', orgId, access: undefined };
  const initial: TraceSearch = { actor: picked.id, include_agents: true, kind: 'work', action: 'thread.started', group: 'kind' };
  const changes: TraceSearch[] = []; people = () => Promise.resolve([replacement]);
  function Trace() {
    const [filters, setFilters] = useState(initial);
    const person = filters.actor === picked.id ? picked : filters.actor === replacement.id ? replacement : null;
    return <TraceFilters scope={scope} filters={filters} person={person} onChange={(next) => { changes.push(next); setFilters(next); }} />;
  }
  draw(<Trace />); expect(personInput(i18n.t('trace.person'))).toHaveValue(picked.display_name); expect(peopleCalls()).toHaveLength(0);
  await user.click(screen.getByRole('button', { name: i18n.t('trace.clearPerson') }));
  expect(changes.at(-1)).toEqual({ ...initial, actor: undefined });
  await choose(replacement, i18n.t('trace.person'));
  const latest = changes.at(-1); if (latest === undefined) throw new Error('Selecting a person must update Trace filters.');
  expect(latest).toEqual({ ...initial, actor: replacement.id });
  expect(peopleCalls()[0]?.[1].params.query).toEqual({ q: replacement.username, org_id: orgId });
  expect(wireFilters(latest)).toMatchObject({ actor: replacement.id, include_agents: 'true', kind: 'work', action: 'thread.started' });
  const url = new URL(traceHref(scope, latest.actor, latest), 'http://example.test');
  expect(url.pathname).toBe(`/trace/${threadId}`); expect(url.searchParams.get('actor')).toBe(replacement.id); expect(url.searchParams.get('include_agents')).toBe('true');
  expect(screen.getByRole('checkbox', { name: i18n.t('trace.includeAgents') })).toBeChecked(); expect(calls.writes).not.toHaveBeenCalled();
});


it('can locate an exact actor for a directly shared thread without asking to view its parent org', async () => {
  history = ({ query }) => Promise.resolve(query.actor === picked.id ? page('Direct-share actor history') : page('Direct-share history'));
  people = (q, org) => org !== undefined ? Promise.reject(failure('authz.denied')) : Promise.resolve(q === picked.username ? [picked] : []);
  draw(<Frame />); await screen.findByText('Direct-share history'); await choose();
  await screen.findByText('Direct-share actor history');
  expect(peopleCalls()[0]?.[1].params.query).toEqual({ q: picked.username });
  expect(historyCalls().at(-1)?.[1].params).toMatchObject({ path: { object: 'nodes', id: threadId }, query: { actor: picked.id } });
  expect(calls.writes).not.toHaveBeenCalled();
});

it('keeps composing, empty and dismissed Enter inside the person locator without submitting its form', async () => {
  const choosePerson = vi.fn(), submit = vi.fn();
  draw(<form onSubmit={(event) => { event.preventDefault(); submit(); }}>
    <PersonLocator label="Person locator" value={null} person={null} onChoose={choosePerson} />
    <button type="submit">Commit</button>
  </form>);
  await user.click(personInput('Person locator')); await user.keyboard('{Enter}');
  expect(choosePerson).not.toHaveBeenCalled(); expect(submit).not.toHaveBeenCalled();
  const input = await search(picked.username, 'Person locator');
  await screen.findByRole('option', { name: picked.display_name });
  fireEvent.keyDown(input, { key: 'Enter', code: 'Enter', isComposing: true, keyCode: 229 });
  expect(choosePerson).not.toHaveBeenCalled(); expect(submit).not.toHaveBeenCalled();
  await user.keyboard('{Escape}{Enter}');
  expect(choosePerson).not.toHaveBeenCalled(); expect(submit).not.toHaveBeenCalled();
  await user.click(input); await user.keyboard('{Enter}');
  expect(choosePerson).toHaveBeenCalledExactlyOnceWith(picked); expect(submit).not.toHaveBeenCalled();
  expect(calls.writes).not.toHaveBeenCalled();
});
