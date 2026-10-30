/** Search keeps its visible filter groups, category choices and result actions consistent across languages and themes. */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as SearchApi from '@/api/search';
import { ApiError, client } from '@/api/client';
import type * as NodesApi from '@/api/nodes';
import type * as TraceApi from '@/api/trace';
import type * as ReferenceApi from '@/api/references';
import type * as Result from '@/features/search/ResultView';
import type * as Screen from '@/shell/screen';
import { SearchPage } from '@/features/search/Search';
import type { SearchParams } from '@/features/search/params';
import {keyOfHit} from '@/features/search/hits';
import type { NameHit, NodeRef, SearchHit, SearchHitPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { org } from '../doubles/fixtures';
import { userMessage } from '../doubles/thread-rows';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';
const calls = vi.hoisted(() => ({ read: vi.fn<(_filters: Parameters<typeof SearchApi.searchQuery>[0]) => Promise<SearchHitPage>>(), node: vi.fn(), names: vi.fn<(..._args: Parameters<typeof SearchApi.namesQuery>) => NameHit[]>(), preview: vi.fn<(_kind: string, _id: string) => void>(), go: vi.fn(), beside: vi.fn<typeof Result.openResult>() }));
const ref = (n: number, kind: NodeRef['kind'], name: string): NodeRef => ({ id: `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`, kind, name, redacted: false });
const root = ref(1, 'org', 'Acme');
const project = ref(2, 'project', 'Web');
const topic = ref(3, 'topic', 'Login');
const thread = ref(4, 'thread', 'Release');
const hit: SearchHit = { kind: 'item', item: userMessage('search-item', 'redirect route', { author: ADA }), thread, topic, project, snippet: 'redirect route', matches: [[0, 8]] };
vi.mock('@/api/search', async (original) => {
  const api = await original<typeof SearchApi>();
  return { ...api, streamSearch: async (filters: Parameters<typeof SearchApi.searchQuery>[0], receive: Parameters<typeof SearchApi.streamSearch>[1]) => {
    const page = await calls.read(filters);
    receive({type: 'options', page_size: 20, reorder_window_secs: 5, meaning_pause_ms: 800});
    for (const hit of page.items) receive({type: 'hit', id: keyOfHit(hit), hit, found_by: 'words'});
    receive({type: 'order', received: page.items.length, items: page.items.map((hit, index) => ({id: keyOfHit(hit), rank: 1 / (index + 61), found_by: 'words'}))});
    receive({type: 'source', source: 'words', state: 'done', reason: null, received: page.items.length});
    receive({type: 'source', source: 'meaning', state: 'done', reason: null, received: 0});
    receive({type: 'cursor', cursor: page.next_cursor});
  }, searchQuery: (filters: Parameters<typeof SearchApi.searchQuery>[0]) => ({ ...api.searchQuery(filters), queryFn: () => calls.read(filters) }),
    namesQuery: (...args: Parameters<typeof SearchApi.namesQuery>) => ({ ...api.namesQuery(...args), queryFn: () => Promise.resolve({ items: calls.names(...args), partial: false }) }),
  };
});
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(), orgsQuery: { queryKey: ['compact-search-orgs'], queryFn: () => Promise.resolve([]) },
  nodeQuery: (id: string) => ({ queryKey: ['compact-search-node', id], queryFn: () => { calls.node(id); return Promise.resolve(org(root.id, 'acme', 'Acme', ADA)); } }),
}));
vi.mock('@/api/trace', async (original) => ({ ...await original<typeof TraceApi>(), selectedPersonQuery: (id: string) => ({
  queryKey: ['compact-person', id], enabled: id !== '', queryFn: () => Promise.resolve(ADA),
}) }));
vi.mock('@/api/references', async (original) => ({ ...await original<typeof ReferenceApi>(), referencePreviewQuery: (reference: Parameters<typeof ReferenceApi.referencePreviewQuery>[0]) => ({
  queryKey: ['compact-preview', reference.kind, reference.id], queryFn: () => {
    calls.preview(reference.kind, reference.id);
    return Promise.resolve({kind:'document' as const,id:reference.id,node_kind:null,title:'Selected document',place:null,description:'',updated_at:null,updated_by:null,part:null,file:null});
  },
}) }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useRouter: () => ({ history: { push: calls.go } }) }));
vi.mock('@/features/search/ResultView', async (original) => ({ ...await original<typeof Result>(), openResult: (...args: Parameters<typeof Result.openResult>) => calls.beside(...args) }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
const sessionBefore = client.session.getState();
function Page({ initial = {} }: { initial?: SearchParams }) {
  const [params, setParams] = useState(initial);
  return <SearchPage params={params} onParams={setParams} />;
}
beforeEach(() => { vi.clearAllMocks(); calls.names.mockReturnValue([]); calls.read.mockResolvedValue({ items: [hit], next_cursor: null }); });
afterEach(async () => {
  cleanup(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', darkBefore);
  client.session.setState(sessionBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});
it.each(modes)('shows grouped filters and categories, preserves choices and offers beside as an icon in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const view = show(<Page />);
  const form = view.container.querySelector<HTMLElement>('form[role="search"]')!;
  const field = within(form).getByRole('textbox', { name: i18n.t('search.field.label') });
  const sidebar = view.container.querySelector('aside');
  expect(sidebar).not.toBeNull();
  expect(within(sidebar!).getByRole('combobox', { name: i18n.t('search.filters.place') })).toBeInTheDocument();
  expect(within(sidebar!).getByRole('checkbox', { name: i18n.t('people.withFile') })).not.toBeChecked();
  const tablist = view.container.querySelector<HTMLElement>('[role="tablist"]')!;
  for (const tab of ['all', 'conversations', 'chats', 'documents', 'files'] as const) {
    expect(within(tablist).getByRole('tab', { name: i18n.t(`search.tabs.${tab}` as const) })).toBeInTheDocument();
  }
  const header = view.container.querySelector('header')!;
  expect(within(header).getByRole('heading', { name: i18n.t('screens.search.title'), level: 1 }).closest('header')?.querySelector('p')).toBeNull();
  expect(calls.read).not.toHaveBeenCalled(); expect(calls.node).not.toHaveBeenCalled(); expect(calls.names).not.toHaveBeenCalled();
  fireEvent.change(field, { target: { value: 'redirect' } });
  fireEvent.submit(form);
  const results = view.container.querySelector<HTMLElement>('section[aria-busy]')!;
  expect(results).toHaveAttribute('aria-label', i18n.t('search.results'));
  await within(results).findByText('route', { exact: false });
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
  expect(calls.read).toHaveBeenLastCalledWith(expect.objectContaining({ q: 'redirect' }));
  expect(within(results).queryByText(i18n.t('search.beside.open'))).not.toBeInTheDocument();
  const beside = within(results).getByRole('button', { name: i18n.t('search.beside.open') });
  expect(beside.textContent).toBe('');
  fireEvent.click(beside);
  expect(calls.beside).toHaveBeenCalledExactlyOnceWith({ kind: 'thread', id: thread.id, anchor: 'search-item', continuationId: null }, { to: '/thread/$threadId', params: { threadId: thread.id }, search: { item: 'search-item' } });
  expect(calls.go).not.toHaveBeenCalled();
  const filters = within(form).getByRole('button', { name: i18n.t('search.filters.button') });
  filters.focus(); fireEvent.click(filters);
  const panel = screen.getByRole('dialog', { name: i18n.t('search.filters.title') });
  expect(within(panel).getByRole('combobox', { name: i18n.t('search.filters.place') })).toBeVisible();
  fireEvent.click(within(panel).getByRole('checkbox', { name: i18n.t('people.withFile') }));
  await waitFor(() => expect(calls.read).toHaveBeenLastCalledWith(expect.objectContaining({ q: 'redirect', with_file: true })));
  fireEvent.keyDown(panel, {key:'Escape',code:'Escape'});
  await waitFor(() => expect(panel).not.toBeInTheDocument());
  expect(filters).toHaveTextContent(i18n.t('search.filters.active', { count: 1 }));
  expect(within(results.parentElement!).getByRole('button', { name: i18n.t('search.filters.clearOne', { what: i18n.t('people.withFile') }) })).toBeInTheDocument();
  await waitFor(() => expect(filters).toHaveFocus());
  expect(field).toHaveValue('redirect');
  fireEvent.click(filters);
  expect(within(screen.getByRole('dialog', { name: i18n.t('search.filters.title') })).getByRole('checkbox', { name: i18n.t('people.withFile') })).toBeChecked();
  expect(calls.node).not.toHaveBeenCalled(); expect(calls.names).not.toHaveBeenCalled();
});
it('keeps a scoped permalink active and reads its visible label once', async () => {
  const view = show(<Page initial={{ q: 'redirect', scope: root.id, kinds: 'user_message', participating: 'true', sort: 'relevance' }} />);
  await waitFor(() => expect(calls.read).toHaveBeenCalledWith(expect.objectContaining({ q: 'redirect', scope: root.id, kinds: ['user_message'], participating: true, sort: 'relevance' })));
  const sidebar = view.container.querySelector('aside')!;
  await waitFor(() => expect(within(sidebar).getByRole('combobox', { name: i18n.t('search.filters.place') })).toHaveValue('Acme'));
  expect(calls.node).toHaveBeenCalledExactlyOnceWith(root.id);
  expect(screen.getByRole('tab', { name: i18n.t('search.tabs.conversations') })).toHaveAttribute('data-state', 'active');
  await userEvent.click(within(sidebar).getByText(i18n.t('search.filters.conversationKinds'), { selector: 'summary' }));
  expect(within(sidebar).getByRole('checkbox', { name: i18n.t('search.kinds.user_message') })).toBeChecked();
  expect(calls.names).not.toHaveBeenCalled();
});

it('keeps Conversations selected when the last kind in an older link is switched off', async () => {
  const view = show(<Page initial={{ q: 'redirect', kinds: 'user_message' }} />);
  const sidebar = view.container.querySelector('aside')!;
  await waitFor(() => expect(calls.read).toHaveBeenCalledWith(expect.objectContaining({ kinds: ['user_message'] })));
  fireEvent.click(within(sidebar).getByText(i18n.t('search.filters.conversationKinds'), { selector: 'summary' }));
  fireEvent.click(within(sidebar).getByRole('checkbox', { name: i18n.t('search.kinds.user_message') }));
  await waitFor(() => expect(calls.read).toHaveBeenLastCalledWith(expect.objectContaining({ kinds: [] })));
  expect(screen.getByRole('tab', { name: i18n.t('search.tabs.conversations') })).toHaveAttribute('data-state', 'active');
  fireEvent.click(screen.getByRole('tab', { name: i18n.t('search.tabs.files') }));
  fireEvent.click(screen.getByRole('tab', { name: i18n.t('search.tabs.conversations') }));
  await waitFor(() => expect(calls.read).toHaveBeenLastCalledWith(expect.objectContaining({ kinds: [] })));
  expect(within(sidebar).getByRole('checkbox', { name: i18n.t('search.kinds.user_message') })).not.toBeChecked();
});

it('reads only a bounded preview to label an exact document selection', async () => {
  const document = '00000000-0000-4000-8000-000000000123';
  const view = show(<Page initial={{q:'redirect',document_id:document}} />);
  const sidebar = view.container.querySelector('aside')!;
  await waitFor(() => expect(within(sidebar).getByRole('combobox', {name:i18n.t('search.filters.place')})).toHaveValue('Selected document'));
  expect(calls.preview).toHaveBeenCalledExactlyOnceWith('document', document);
  expect(calls.read).toHaveBeenCalledWith(expect.objectContaining({document_id:document}));
});

it('clears filters without changing a legacy category or order', async () => {
  show(<Page initial={{ q: 'redirect', kinds: 'document', with_file: 'true', sort: 'relevance' }} />);
  await waitFor(() => expect(calls.read).toHaveBeenCalledWith(expect.objectContaining({kinds:['document'], with_file:true, sort:'relevance'})));
  fireEvent.click(screen.getByRole('button', {name:i18n.t('search.filters.clearAll')}));
  expect(screen.getByRole('tab', {name:i18n.t('search.tabs.documents')})).toHaveAttribute('data-state', 'active');
  await waitFor(() => expect(calls.read).toHaveBeenLastCalledWith(expect.objectContaining({q:'redirect', kinds:['document'], sort:'relevance'})));
  expect(calls.read.mock.lastCall?.[0].with_file).toBeUndefined();
});

it.each(['column', 'sheet'] as const)('filters by owner and edited date in the %s', async (layout) => {
  calls.names.mockReturnValue([{ kind: 'user', id: ADA.id, name: ADA.display_name, username: ADA.username, avatar: ADA.avatar,
    path: [], owner: null, archived: false, recent: false, color: null, custom_color: null, address: null, tag: null, thread_attention: null }]);
  const view = show(<Page initial={{ q: 'redirect' }} />);
  let filters = view.container.querySelector('aside')!;
  if (layout === 'sheet') {
    fireEvent.click(screen.getByRole('button', { name: i18n.t('search.filters.button') }));
    filters = screen.getByRole('dialog', { name: i18n.t('search.filters.title') });
  }
  fireEvent.focus(within(filters).getByRole('combobox', { name: i18n.t('search.filters.owner') }));
  fireEvent.click(await screen.findByRole('option', { name: /Ada/ }));
  fireEvent.change(within(filters).getByLabelText(i18n.t('search.filters.from')), { target: { value: '2026-10-01' } });
  await waitFor(() => {
    expect(calls.read).toHaveBeenLastCalledWith(expect.objectContaining({ q: 'redirect', owner: ADA.id, date_field: 'edited' }));
    expect(typeof calls.read.mock.lastCall?.[0].from).toBe('string');
  });
  if (layout === 'sheet') fireEvent.keyDown(filters, {key:'Escape',code:'Escape'});
  const owner = i18n.t('search.filters.activeValue', {label: i18n.t('search.filters.owner'), value: ADA.display_name});
  const results = view.container.querySelector('section[aria-busy]')!.parentElement!;
  expect(await within(results).findByRole('button', { name: i18n.t('search.filters.clearOne', { what: owner }) })).toBeInTheDocument();
  expect(within(results).getByRole('button', { name: /Clear: Edited:/ })).toBeInTheDocument();
});

it('remembers open filter groups on this device in the column and sheet', async () => {
  client.session.setState({userId: 'search-groups-reader'});
  const view = show(<Page />);
  const sidebar = view.container.querySelector('aside')!;
  const who = within(sidebar).getByText(i18n.t('search.filters.whoGroup'), {selector:'summary'});
  expect(who.closest('details')).toHaveAttribute('open');
  await userEvent.click(who);
  await waitFor(() => expect(who.closest('details')).not.toHaveAttribute('open'));
  expect(JSON.parse(readViewChoice('search-groups-reader', 'search', 'filter-groups', ''))).not.toContain('who');
  view.unmount();
  show(<Page />);
  const summary = screen.getByText(i18n.t('search.filters.whoGroup'), {selector:'summary'});
  expect(summary.closest('details')).not.toHaveAttribute('open');
  await userEvent.click(screen.getByRole('button', {name:i18n.t('search.filters.button')}));
  const dialog = screen.getByRole('dialog', {name:i18n.t('search.filters.title')});
  expect(within(dialog).getByText(i18n.t('search.filters.whoGroup'), {selector:'summary'}).closest('details')).not.toHaveAttribute('open');
});

it('uses the default filter groups when a saved group is no longer offered', () => {
  client.session.setState({userId: 'search-old-groups-reader'});
  writeViewChoice('search-old-groups-reader', 'search', 'filter-groups', JSON.stringify(['removed']));
  const view = show(<Page />);
  const sidebar = view.container.querySelector('aside')!;
  expect(within(sidebar).getByText(i18n.t('search.filters.whoGroup'), {selector:'summary'}).closest('details')).toHaveAttribute('open');
});

it.each(modes)('shows a refused search as its real error and retries without losing the query in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  calls.read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:not_found', code: 'not_found', status: 404, title: 'Not found', detail: 'The requested search scope was removed.', request_id: 'search-refusal' }, null));
  show(<Page initial={{ q: 'redirect' }} />);
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent(i18n.t('errors.not_found'));
  expect(screen.getByRole('textbox', { name: i18n.t('search.field.label') })).toHaveValue('redirect');
  expect(calls.read).toHaveBeenCalledTimes(1);
  await userEvent.click(within(alert).getByRole('button', { name: i18n.t('common.tryAgain') }));
  await screen.findByText('route', { exact: false });
  expect(calls.read).toHaveBeenCalledTimes(2);
  expect(calls.read).toHaveBeenLastCalledWith(expect.objectContaining({ q: 'redirect' }));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});
