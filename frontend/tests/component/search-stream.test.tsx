/** Streamed search follows server order through independent arrivals, reading, settling, continuation and typing. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { StrictMode, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SearchPage } from '@/features/search/Search';
import type { SearchParams } from '@/features/search/params';
import type { SearchFilters } from '@/api/search';
import type { SearchHit, SearchStreamEvent as Event } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { ApiError, client } from '@/api/client';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { show } from './support';
const calls = vi.hoisted(() => ({failures: [] as Error[], personFailure: null as Error | null, abortAware: false, requests: [] as {filters: SearchFilters; receive: (event: Event) => void; signal: AbortSignal; cursor?: string; immediate?: boolean}[]}));
vi.mock('@/api/search', async (original) => ({...await original<Record<string, unknown>>(), streamSearch: (filters: SearchFilters, receive: (event: Event) => void, signal: AbortSignal, cursor?: string, immediate?: boolean) => {
  calls.requests.push({filters, receive, signal, ...(cursor === undefined ? {} : {cursor}), ...(immediate === undefined ? {} : {immediate})});
  const failure = calls.failures.shift();
  if (failure !== undefined) return Promise.reject(failure);
  return new Promise<void>((_resolve, reject) => {
    if (calls.abortAware) signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')), {once: true});
  });
}}));
vi.mock('@/api/nodes', async (original) => ({...await original<Record<string, unknown>>(), orgsQuery: {queryKey: ['stream-orgs'], queryFn: () => []}}));
vi.mock('@/api/trace', async (original) => ({...await original<Record<string, unknown>>(), selectedPersonQuery: (id: string) => ({
  queryKey: ['stream-person', id], enabled: id !== '', queryFn: () => calls.personFailure === null ? Promise.resolve({id, kind: 'human', username: 'ada', display_name: 'Ada', avatar: {text: 'A', color: 1}}) : Promise.reject(calls.personFailure),
})}));
vi.mock('@/shell/screen', () => ({useScreenHeading: () => undefined}));
vi.mock('@/features/search/Hit', () => ({Hit: ({hit,matchField}: {hit: SearchHit;matchField?:string}) => <li data-match-field={matchField}><button>{hit.snippet}</button></li>}));
function Page({initial = {q: 'release', sort: 'relevance'}}: {initial?: SearchParams}) {
  const [params, setParams] = useState(initial);
  return <SearchPage params={params} onParams={setParams} />;
}
const payload = (id: string): SearchHit => ({kind: 'name', node: {id, name: id, kind: 'topic', redacted: false}, path: [], href: `/topic/${id}`, at: '2026-01-01T00:00:00Z', snippet: id, matches: [[0, 1]]});
const hit = (id: string, found_by: 'words' | 'meaning' = 'words'): Event => ({type: 'hit', id, hit: payload(id), found_by});
const order = (...ids: string[]): Event => ({type: 'order', received: ids.length, items: ids.map((id, index) => ({id, rank: 100 - index, found_by: id === 'meaning' ? 'meaning' : 'words'}))});
const source = (name: string, state = 'done', reason: string | null = null): Event => ({type: 'source', source: name, state, reason, received: 0});
async function send(...events: Event[]) {await act(async () => {for (const event of events) calls.requests.at(-1)?.receive(event); await Promise.resolve();});}
const list = () => within(screen.getByRole('region', {name: i18n.t('search.results')})).getByRole('list');
const rows = () => within(list()).getAllByRole('button').map((button) => button.textContent);
const originalSession = client.session.getState();
beforeEach(() => {calls.requests.length = 0; calls.failures.length = 0; calls.personFailure = null; calls.abortAware = false;});
afterEach(() => {cleanup(); vi.useRealTimers(); client.session.setState(originalSession);});
async function start(initial?: SearchParams, pageSize = 2) {
  const view = show(<Page {...(initial === undefined ? {} : {initial})} />);
  await waitFor(() => expect(calls.requests).toHaveLength(1));
  await send({type: 'options', page_size: pageSize, reorder_window_secs: 5, meaning_pause_ms: 800}, source('words', 'running'), source('meaning', 'running'));
  return view;
}
it('starts a linked search through strict-mode remounts and cancels it when the view closes', async () => {
  const view = show(<StrictMode><Page /></StrictMode>);
  await waitFor(() => expect(calls.requests.length).toBeGreaterThan(0));
  await waitFor(() => expect(calls.requests.at(-1)?.signal.aborted).toBe(false));
  await send({type: 'options', page_size: 20, reorder_window_secs: 5, meaning_pause_ms: 800}, hit('words'), order('words'), source('words'), source('meaning'));
  expect(rows()).toEqual(['words']);
  const signal = calls.requests.at(-1)?.signal;
  view.unmount();
  await waitFor(() => expect(signal?.aborted).toBe(true));
});
it('restarts an interrupted search when its view reopens with the same query', async () => {
  calls.abortAware = true;
  const view = show(<Page />);
  await waitFor(() => expect(calls.requests).toHaveLength(1));
  await send({type:'options',page_size:20,reorder_window_secs:5,meaning_pause_ms:800}, hit('partial'), order('partial'));
  expect(rows()).toEqual(['partial']);
  const first = calls.requests[0]?.signal;
  view.rerender(<div />);
  await waitFor(() => expect(first?.aborted).toBe(true));
  view.rerender(<Page />);
  await waitFor(() => expect(calls.requests).toHaveLength(2));
  await send({type:'options',page_size:20,reorder_window_secs:5,meaning_pause_ms:800}, hit('words'), order('words'), source('words'), source('meaning'));
  expect(rows()).toEqual(['words']);
});
it('resumes sort and filters on this device without restoring the words typed earlier', async () => {
  client.session.setState({userId: 'search-return-reader'});
  const view = await start({q: 'private earlier words', sort: 'relevance', with_file: 'true', regex: 'true'});
  view.unmount(); calls.requests.length = 0;
  show(<Page initial={{}} />);
  expect(screen.getByRole('textbox', {name: i18n.t('search.field.label')})).toHaveValue('');
  expect(screen.getByRole('button', {name: i18n.t('search.filters.active', {count: 2})})).toBeVisible();
  expect(calls.requests).toHaveLength(0);
  const stored = JSON.parse(readViewChoice('search-return-reader', 'search', 'filters', '')) as Record<string, unknown>;
  expect(stored).toEqual({sort: 'relevance', regex: 'true', with_file: 'true'});
  expect(stored.q).toBeUndefined();
});
it('uses an explicit search link before the saved device view and retains a later Back change', async () => {
  client.session.setState({userId: 'search-link-reader'});
  writeViewChoice('search-link-reader', 'search', 'filters', JSON.stringify({sort: 'relevance', with_file: 'true'}));
  const view = show(<SearchPage params={{q: 'release', sort: 'newest'}} onParams={vi.fn()} />);
  await waitFor(() => expect(calls.requests).toHaveLength(1));
  expect(calls.requests[0]?.filters).toMatchObject({q: 'release', sort: 'newest'});
  expect(calls.requests[0]?.filters.with_file).toBeUndefined();
  view.rerender(<SearchPage params={{q: 'release', sort: 'relevance', mentions_me: 'true'}} onParams={vi.fn()} />);
  await waitFor(() => expect(calls.requests).toHaveLength(2));
  expect(calls.requests[1]?.filters).toMatchObject({sort: 'relevance', mentions_me: true});
});
it('keeps cleared filters cleared on return and never restores another reader’s view', async () => {
  client.session.setState({userId: 'search-clear-reader'});
  writeViewChoice('search-clear-reader', 'search', 'filters', JSON.stringify({sort: 'relevance', with_file: 'true'}));
  const view = show(<SearchPage params={{q: 'release', sort: 'newest', with_file: 'true'}} onParams={vi.fn()} />);
  await waitFor(() => expect(calls.requests).toHaveLength(1));
  expect(calls.requests[0]?.filters.with_file).toBe(true);
  view.rerender(<SearchPage params={{q: 'release'}} onParams={vi.fn()} />);
  await waitFor(() => expect(calls.requests).toHaveLength(2));
  expect(readViewChoice('search-clear-reader', 'search', 'filters', '')).toBe('{}');
  view.unmount(); calls.requests.length = 0;
  await start({q: 'release'});
  expect(calls.requests[0]?.filters.with_file).toBeUndefined();
  cleanup(); calls.requests.length = 0;
  client.session.setState({userId: 'search-different-reader'});
  await start({q: 'release'});
  expect(calls.requests[0]?.filters.with_file).toBeUndefined();
  expect(calls.requests[0]?.filters.sort).toBeUndefined();
});
it.each(['not json', '[]', '{"sort":"gone","scope":"removed","q":"old text"}'])('uses the current default for unavailable saved search values (%s)', async (saved) => {
  client.session.setState({userId: `search-invalid-${saved}`});
  writeViewChoice(`search-invalid-${saved}`, 'search', 'filters', saved);
  await start({q: 'release'});
  expect(calls.requests[0]?.filters.sort).toBeUndefined();
  expect(calls.requests[0]?.filters.scope).toBeUndefined();
  expect(screen.getByRole('textbox', {name: i18n.t('search.field.label')})).toHaveValue('release');
});
it.each([403, 404])('uses the default when the server refuses a restored scope (%s), preserving the new words', async (status) => {
  client.session.setState({userId: `search-gone-scope-${status}`});
  const scope = '00000000-0000-4000-8000-000000000001';
  writeViewChoice(`search-gone-scope-${status}`, 'search', 'filters', JSON.stringify({scope, sort: 'relevance'}));
  calls.failures.push(new ApiError({type: 'orch:error:not_found', code: 'not_found', status, title: 'Not found', detail: 'The search scope is unavailable.', request_id: 'gone-search-scope'}, null));
  show(<Page initial={{q: 'new words'}} />);
  await waitFor(() => expect(calls.requests).toHaveLength(2));
  expect(calls.requests[0]?.filters.scope).toBe(scope);
  expect(calls.requests[1]?.filters).toMatchObject({q: 'new words'});
  expect(calls.requests[1]?.filters.scope).toBeUndefined();
  expect(calls.requests[1]?.filters.sort).toBeUndefined();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(readViewChoice(`search-gone-scope-${status}`, 'search', 'filters', '')).toBe('{}');
});
it.each([403, 404])('clears an unavailable restored person without discarding the other search choices (%s)', async (status) => {
  client.session.setState({userId: `search-gone-owner-${status}`});
  const owner = '00000000-0000-4000-8000-000000000001';
  writeViewChoice(`search-gone-owner-${status}`, 'search', 'filters', JSON.stringify({owner, sort: 'relevance'}));
  calls.personFailure = new ApiError({type:'orch:error:not_found',code:'not_found',status,title:'Not found',detail:'Unavailable',request_id:'gone-person'}, null);
  show(<Page initial={{q:'new words'}} />);
  await waitFor(() => expect(calls.requests.some((request) => request.filters.owner === owner)).toBe(true));
  await waitFor(() => expect(calls.requests.at(-1)?.filters.owner).toBeUndefined());
  expect(calls.requests.at(-1)?.filters).toMatchObject({q:'new words',sort:'relevance'});
  expect(JSON.parse(readViewChoice(`search-gone-owner-${status}`, 'search', 'filters', ''))).toEqual({sort:'relevance'});
});
it('keeps an explicit unavailable person without revealing a name in Chinese', async () => {
  const previous = currentLocale();
  await setLocale('zh-CN');
  try {
    const owner = '00000000-0000-4000-8000-000000000001';
    calls.personFailure = new ApiError({type:'orch:error:not_found',code:'not_found',status:404,title:'Not found',detail:'Unavailable',request_id:'explicit-person'}, null);
    show(<Page initial={{q:'new words',owner}} />);
    await screen.findByText('不可用');
    expect(calls.requests.at(-1)?.filters.owner).toBe(owner);
    expect(screen.getByRole('button', {name:'清除：所有者：不可用'})).toBeInTheDocument();
  } finally { cleanup(); await setLocale(previous); }
});
it('retains an explicit refused scope and offers the ordinary retry instead of changing its filters', async () => {
  const scope = '00000000-0000-4000-8000-000000000001';
  calls.failures.push(new ApiError({type: 'orch:error:not_found', code: 'not_found', status: 404, title: 'Not found', detail: 'The search scope is unavailable.', request_id: 'explicit-search-scope'}, null));
  show(<Page initial={{q: 'new words', scope}} />);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(calls.requests).toHaveLength(1);
  expect(calls.requests[0]?.filters.scope).toBe(scope);
  expect(screen.getByRole('button', {name: i18n.t('common.tryAgain')})).toBeEnabled();
});
it('renders words first while meaning runs and follows the final server order', async () => {
  await start(); await send(hit('words'), order('words'), source('words'));
  expect(rows()).toEqual(['words']); expect(screen.getByRole('status')).toHaveTextContent(i18n.t('search.searching', {count: 1, number: '1'}));
  await send(hit('meaning', 'meaning'), order('meaning', 'words'), source('meaning'));
  expect(rows()).toEqual(['meaning', 'words']); expect(screen.getByRole('status')).toHaveTextContent('2 results');
});
it('renders meaning first without waiting for the words source', async () => {
  await start(); await send(hit('meaning', 'meaning'), order('meaning'), source('meaning'));
  expect(rows()).toEqual(['meaning']); expect(screen.getByRole('status')).toHaveTextContent('Searching…');
  await send(hit('words'), order('words', 'meaning'), source('words'));
  expect(rows()).toEqual(['words', 'meaning']);
});
it('accepts a combined batch and source membership without duplicating a payload', async () => {
  await start(); await send(hit('both'), order('both'), {type: 'order', received: 1, items: [{id: 'both', rank: 0.032, found_by: 'both'}]}, source('words'), source('meaning'));
  expect(rows()).toEqual(['both']); expect(screen.getByRole('status')).toHaveTextContent(i18n.t('search.total', {count: 1, number: '1'}));
});
it('keeps a meaning-first payload once while adopting the server’s later word field', async () => {
  await start(); await send(hit('both','meaning'), order('both'), source('meaning'));
  const entry = {id:'both',rank:0.032,found_by:'both' as const,match_field:'description' as const};
  await send({type:'order',received:1,items:[entry]},source('words'));
  expect(rows()).toEqual(['both']);
  expect(within(list()).getByRole('button', {name:'both'}).closest('li')).toHaveAttribute('data-match-field','description');
});
it('keeps words when meaning is unavailable and puts its reason on hover', async () => {
  await start(); await send(hit('words'), order('words'), source('meaning', 'unavailable', 'Query vector timed out'), source('words'));
  expect(rows()).toEqual(['words']); expect(screen.getByText(i18n.t('search.meaningUnavailable'))).toHaveAttribute('title', 'Query vector timed out');
});
for (const locale of ['en', 'zh-CN'] as const) it.each(['provider_disabled', 'provider_removed'] as const)(`keeps cached meaning and names its index pause (%s, ${locale})`, async (reason) => {
  const previous = currentLocale();
  await setLocale(locale);
  try {
    await start();
    await send(hit('meaning', 'meaning'), hit('words'), order('meaning', 'words'), source('words'), source('meaning', 'done', reason));
    expect(rows()).toEqual(['meaning', 'words']);
    const status = screen.getByRole('button', {name: i18n.t('search.indexPaused')});
    expect(status).toHaveAttribute('title', i18n.t(`search.meaningReasons.${reason}`));
    expect(screen.queryByText(i18n.t('search.meaningUnavailable'))).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByRole('button', {name: i18n.t('common.tryAgain')})).toBeNull();
  } finally { cleanup(); await setLocale(previous); }
});
it('keeps words and the provider pause reason when a fresh query cannot be embedded', async () => {
  await start();
  await send(hit('words'), order('words'), source('words'), source('meaning', 'unavailable', 'provider_disabled'));
  expect(rows()).toEqual(['words']);
  expect(screen.getByRole('button', {name: i18n.t('search.meaningUnavailable')})).toHaveAttribute('title', i18n.t('search.meaningReasons.provider_disabled'));
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.queryByRole('button', {name: i18n.t('common.tryAgain')})).toBeNull();
});
it('keeps meaning results when words fail and offers the normal request retry', async () => {
  await start(); await send(hit('meaning', 'meaning'), order('meaning'), source('words', 'unavailable', 'Database unavailable'), source('meaning'));
  expect(rows()).toEqual(['meaning']); expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('search.wordsUnavailable'));
  expect(screen.getByRole('button', {name: i18n.t('common.tryAgain')})).toBeEnabled();
});
it('applies each server batch while settling', async () => {
  await start(); await send(hit('first'), order('first')); expect(rows()).toEqual(['first']);
  await send(hit('second'), order('second', 'first')); expect(rows()).toEqual(['second', 'first']);
});
it.each(['hover', 'focus', 'details'] as const)('holds an order while a result has %s and applies it when reading stops', async (kind) => {
  await start(); await send(hit('first'), order('first'));
  const button = screen.getByRole('button', {name: 'first'});
  if (kind === 'hover') fireEvent.pointerOver(button);
  else if (kind === 'focus') fireEvent.focus(button);
  else {const detail = document.createElement('div'); detail.dataset.searchResult = 'preview'; await act(async () => {document.body.append(detail); await Promise.resolve();});}
  await send(hit('second'), order('second', 'first')); expect(rows()).toEqual(['first']);
  if (kind === 'hover') fireEvent.pointerLeave(list());
  else if (kind === 'focus') fireEvent.blur(button);
  else await act(async () => {document.querySelector('[data-search-result]')?.remove();await Promise.resolve();});
  await waitFor(() => expect(rows()).toEqual(['second', 'first']));
});
it('fixes rows after scrolling past the first screen and appends meaning under a divider', async () => {
  await start(); await send(hit('words'), order('words'));
  const scroll = document.createElement('div'); document.body.append(scroll); Object.defineProperties(scroll, {clientHeight: {value: 400}, scrollTop: {value: 401}});
  fireEvent.scroll(scroll);
  await send(hit('meaning', 'meaning'), order('meaning', 'words'));
  expect(rows()).toEqual(['words', 'meaning']); expect(screen.getByRole('separator')).toHaveTextContent(i18n.t('search.similarMeaning'));
  scroll.remove();
});
it('fixes the first screen when the configured reorder window ends', async () => {
  vi.useFakeTimers({shouldAdvanceTime: true});
  await start(); await send(hit('words'), order('words'));
 await act(async () => {vi.advanceTimersByTime(5001);await Promise.resolve();});
  await send(hit('meaning', 'meaning'), order('meaning', 'words')); expect(rows()).toEqual(['words', 'meaning']);
});
it('reveals all received results on the first Show more and resumes both sources only on later presses', async () => {
  await start(undefined, 1); await send(hit('first'), hit('second'), order('first', 'second'), source('words'), source('meaning'), {type: 'cursor', cursor: 'both-sources'});
  expect(rows()).toEqual(['first']); fireEvent.click(screen.getByRole('button', {name: i18n.t('search.more')}));
  expect(rows()).toEqual(['first', 'second']); expect(calls.requests).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', {name: i18n.t('search.more')}));
  expect(calls.requests).toHaveLength(2); expect(calls.requests[1]?.cursor).toBe('both-sources');
  await send(hit('third'), order('third'), source('words'), source('meaning'), {type: 'cursor', cursor: null});
  expect(rows()).toEqual(['first', 'second', 'third']);
});
it('aborts the old search on a keystroke, debounces words, and asks meaning after a pause or immediately on Enter', async () => {
  await start(); const field = screen.getByRole('textbox', {name: i18n.t('search.field.label')});
  fireEvent.change(field, {target: {value: 'rollback'}}); expect(calls.requests[0]?.signal.aborted).toBe(true);
  expect(calls.requests).toHaveLength(1);
  await waitFor(() => expect(calls.requests.length).toBe(2)); expect(calls.requests[1]?.immediate).toBe(false);
  fireEvent.submit(screen.getByRole('search')); await waitFor(() => expect(calls.requests.at(-1)?.immediate).toBe(true));
});
it('sends every filter in the one search request', async () => {
  await start({q: 'release', sort: 'relevance', author: 'person', scope: 'scope', chat_id: 'chat', from: '2026-01-01', to: '2026-02-01', mentions_me: 'true', with_file: 'true'});
  expect(calls.requests[0]?.filters).toEqual(expect.objectContaining({author: 'person', scope: 'scope', chat_id: 'chat', mentions_me: true, with_file: true, from: '2026-01-01', to: '2026-02-01'}));
});
it.each([['newest', 'newest'], ['relevance', 'phrase']] as const)('says why %s uses words alone (%s)', async (sort, reason) => {
  await start({q: reason === 'phrase' ? '"previous release"' : 'release', sort});
  await send(hit('words'), order('words'), source('words'), source('meaning', 'done', reason));
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t(`search.wordsOnly.${reason}`));
});

it('retires the old results and cancels their stream when the reading authority changes', async () => {
  const {client} = await import('@/api/client');
  const before = client.session.getState().readerVersion;
  try {
    await start(); await send(hit('private'), order('private'));
    expect(rows()).toEqual(['private']);
    await act(async () => {client.session.setState({readerVersion: (before ?? 0) + 1});await Promise.resolve();});
    expect(screen.queryByRole('button', {name: 'private'})).not.toBeInTheDocument();
    expect(calls.requests[0]?.signal.aborted).toBe(true);
    await waitFor(() => expect(calls.requests).toHaveLength(2));
    await act(async () => {calls.requests[0]?.receive(hit('stale')); calls.requests[0]?.receive(order('stale'));await Promise.resolve();});
    expect(screen.queryByRole('button', {name: 'stale'})).not.toBeInTheDocument();
  } finally {cleanup(); client.session.setState({readerVersion: before});}
});

it('accepts an authoritative order before its payloads and stays unfinished until the continuation event', async () => {
  await start();
  await send({type: 'order', received: 1, items: [{id: 'both', rank: 0.03, found_by: 'both'}]}, hit('both'), source('words'), source('meaning'));
  expect(rows()).toEqual(['both']);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('search.searching', {count: 1, number: '1'}));
  await send({type: 'cursor', cursor: null});
  expect(screen.getByRole('status')).not.toHaveTextContent('Searching');
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('search.total', {count: 1, number: '1'}));
});

it.each(['回', '回滚'])('starts one indexed streamed search for a short Chinese query %s', async (q) => {
  await start({q, sort: 'relevance'});
  expect(calls.requests[0]?.filters.q).toBe(q);
  await send(hit(q), order(q), source('words'), source('meaning'), {type: 'cursor', cursor: null});
  expect(rows()).toEqual([q]);
});

it('shows a meaning failure reason on tap and keyboard focus and closes it with Escape',async()=>{
  await start();await send(hit('words'),order('words'),source('meaning','unavailable','query_timeout'),source('words'));
  const reason=screen.getByRole('button',{name:i18n.t('search.meaningUnavailable')});
  fireEvent.click(reason);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('search.meaningReasons.query_timeout'));
  fireEvent.keyDown(reason,{key:'Escape'});
  await waitFor(()=>expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  fireEvent.focus(reason);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('search.meaningReasons.query_timeout'));
});

it('first Show more reveals every received hit even while its latest order is held',async()=>{
  await start(undefined,1);await send(hit('words'),order('words'));
  fireEvent.pointerOver(screen.getByRole('button',{name:'words'}));
  await send(hit('meaning','meaning'),order('meaning','words'),source('words'),source('meaning'),{type:'cursor',cursor:null});
  expect(rows()).toEqual(['words']);
  fireEvent.click(screen.getByRole('button',{name:i18n.t('search.more')}));
  expect(rows()).toEqual(['words','meaning']);
  expect(calls.requests).toHaveLength(1);
});

it('sends an expression as words alone and says why',async()=>{
  await start({q:'rele.se',regex:'true',sort:'relevance'});
  expect(calls.requests[0]?.filters).toMatchObject({q:'rele.se',regex:true});
  await send(hit('words'),order('words'),source('words'),source('meaning','done','regex'),{type:'cursor',cursor:null});
  expect(rows()).toEqual(['words']);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('search.wordsOnly.regex'));
});
it('preserves a document filter in the one request shared by both sources',async()=>{
  const document_id='00000000-0000-4000-8000-000000000123';
  await start({q:'release',document_id,sort:'relevance'});
  expect(calls.requests[0]?.filters).toMatchObject({document_id});
  expect(calls.requests).toHaveLength(1);
});
