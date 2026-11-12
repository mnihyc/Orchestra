/** Current-org searches, deliberate widening, shared addresses and late answers in both languages. */
import userEvent from '@testing-library/user-event';
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import type * as Nodes from '@/api/nodes';
import type * as Search from '@/api/search';
import type * as Router from '@tanstack/react-router';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SearchPage } from '@/features/search/Search';
import { keyOfHit } from '@/features/search/hits';
import type { SearchParams } from '@/features/search/params';
import type { NodeRef, Org, SearchHit } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useSelectedOrg } from '@/shell/org';
import { client } from '@/api/client';
import { readViewChoice } from '@/lib/viewChoices';
import { org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const calls = vi.hoisted(() => ({
  read: vi.fn<(filters: Search.SearchFilters) => Promise<{ hits: SearchHit[]; cursor?: string | null }>>(),
  orgs: [] as Org[],
}));
const work = '00000000-0000-4000-8000-000000000001';
const personal = '00000000-0000-4000-8000-000000000002';
const root = (id: string, name: string): NodeRef => ({ id, name, kind: 'org', redacted: false });
const hit = (id: string, home: NodeRef): SearchHit => ({ kind: 'name', node: { id, name: id, kind: 'topic', redacted: false }, path: [home], href: `/topic/${id}`, at: '2026-01-01T00:00:00Z', snippet: id, matches: [] });
const office = hit('Office canary', root(work, 'Work'));
const own = hit('Personal canary', root(personal, 'Personal'));
vi.mock('@/api/search', async (original) => ({ ...await original<typeof Search>(), streamSearch: async (filters: Search.SearchFilters, receive: Parameters<typeof Search.streamSearch>[1]) => {
  const page = await calls.read(filters);
  receive({ type: 'options', page_size: 20, reorder_window_secs: 0, meaning_pause_ms: 800 });
  for (const item of page.hits) receive({ type: 'hit', id: keyOfHit(item), hit: item, found_by: 'words' });
  receive({ type: 'order', received: page.hits.length, items: page.hits.map((item) => ({ id: keyOfHit(item), rank: 1, found_by: 'words' })) });
  for (const source of ['words', 'meaning']) receive({ type: 'source', source, state: 'done', reason: null, received: page.hits.length });
  receive({ type: 'cursor', cursor: page.cursor ?? null });
} }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(),
  orgsQuery: { queryKey: ['org-scope-roots'], queryFn: () => calls.orgs },
  nodeQuery: (id: string) => ({ queryKey: ['org-scope-node', id], queryFn: () => Promise.resolve(org(id, 'outside', 'Outside', ADA, { org_id: personal })) }),
}));
vi.mock('@/shell/screen', () => ({ useScreenHeading: () => undefined }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useRouter: () => ({ history: { push: vi.fn() } }) }));
function Page({ initial = { q: 'canary' } }: { initial?: SearchParams }) {
  const [params, setParams] = useState(initial);
  return <><output data-address>{JSON.stringify(params)}</output><SearchPage params={params} onParams={setParams} /></>;
}
const selectedBefore = useSelectedOrg.getState();
const sessionBefore = client.session.getState();
beforeEach(() => {
  vi.clearAllMocks();
  client.session.setState({ userId: 'search-org-controls' });
  useSelectedOrg.setState({ orgId: work });
  calls.orgs = [org(work, 'work', 'Work', ADA), org(personal, 'personal', 'Personal', ADA, { org_kind: 'personal' })];
  calls.read.mockImplementation((filters) => Promise.resolve({ hits: filters.scope === work ? [office] : filters.scope === personal ? [own] : [own, office] }));
});
afterEach(async () => { cleanup(); useSelectedOrg.setState(selectedBefore); client.session.setState(sessionBefore); await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('excludes personal hits by default, widens explicitly and restores no global default in %s', async (locale) => {
  await setLocale(locale);
  const view = show(<Page />);
  const results = screen.getByRole('region', { name: i18n.t('search.results') });
  await within(results).findAllByRole('link', { name: 'Office canary' });
  expect(within(results).queryAllByRole('link', { name: 'Personal canary' })).toEqual([]);
  expect(calls.read).toHaveBeenCalledWith(expect.objectContaining({ scope: work }));
  await waitFor(() => expect(view.container.querySelector('[data-address]')).toHaveTextContent(`"org":"${work}"`));
  await userEvent.click(screen.getByRole('button', { name: `${i18n.t('search.org.label')}: ${i18n.t('search.org.in', { org: 'Work' })}` }));
  await userEvent.click(await screen.findByRole('menuitemradio', { name: i18n.t('search.org.all') }));
  await within(results).findAllByRole('link', { name: 'Personal canary' });
  expect(calls.read.mock.lastCall?.[0].scope).toBeUndefined();
  expect(view.container.querySelector('[data-address]')).toHaveTextContent('"org":"all"');
  expect(view.container.querySelectorAll('[data-search-org-heading]')).toHaveLength(2);
  expect(view.container.querySelectorAll('[data-search-org]')).toHaveLength(2);
  expect(JSON.parse(readViewChoice('search-org-controls', 'search', 'filters', '{}'))).not.toHaveProperty('org');
  view.unmount();
  show(<Page />);
  await waitFor(() => expect(calls.read.mock.lastCall?.[0].scope).toBe(work));
  expect(screen.queryAllByRole('link', { name: 'Personal canary' })).toEqual([]);
});
it.each(['en', 'zh-CN'] as const)('keeps a shared all-org address, omits single-root grouping and offers a bounded empty-state count in %s', async (locale) => {
  await setLocale(locale);
  calls.read.mockImplementation((filters) => Promise.resolve({ hits: filters.scope === undefined ? [own] : [], cursor: filters.scope === undefined ? 'more-readable-results' : null }));
  const view = show(<Page />);
  const offer = await screen.findByRole('button', { name: i18n.t('search.org.searchAllCount', { number: '1+' }) });
  fireEvent.click(offer);
  await screen.findAllByRole('link', { name: 'Personal canary' });
  expect(calls.read.mock.calls.filter(([filters]) => filters.scope === undefined)).toHaveLength(2);
  expect(view.container.querySelectorAll('[data-search-org-heading]')).toHaveLength(0);
  expect(view.container.querySelectorAll('[data-search-org]')).toHaveLength(1);
  view.unmount();
  useSelectedOrg.setState({ orgId: personal });
  show(<Page initial={{ q: 'canary', org: 'all' }} />);
  await screen.findAllByRole('link', { name: 'Personal canary' });
  expect(calls.read.mock.lastCall?.[0].scope).toBeUndefined();
});
it('offers no All my orgs choice with one root and never sends a contradictory root/place address', async () => {
  calls.orgs = calls.orgs.slice(0, 1);
  const view = show(<Page initial={{ q: 'canary', org: work, scope: personal }} />);
  expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('search.org.outsidePlace'));
  expect(calls.read).not.toHaveBeenCalled();
  const choice = screen.getByRole('button', { name: `${i18n.t('search.org.label')}: ${i18n.t('search.org.in', { org: 'Work' })}` });
  expect(choice).toBeDisabled();
  expect(screen.queryByText(i18n.t('search.org.all'), { exact: true })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: i18n.t('search.org.clearPlace') }));
  await waitFor(() => expect(calls.read).toHaveBeenCalledWith(expect.objectContaining({ scope: work })));
  expect(view.container.querySelector('[data-address]')).not.toHaveTextContent('"scope"');
});
it('a fresh address follows a changed rail root and ignores the old delayed stream', async () => {
  let finish: ((value: { hits: SearchHit[] }) => void) | undefined;
  calls.read.mockImplementation((filters) => filters.scope === work ? new Promise((resolve) => { finish = resolve; }) : Promise.resolve({ hits: [own] }));
  const view = show(<SearchPage params={{ q: 'canary' }} onParams={vi.fn()} />);
  await waitFor(() => expect(calls.read).toHaveBeenCalledWith(expect.objectContaining({ scope: work })));
  act(() => useSelectedOrg.setState({ orgId: personal }));
  await screen.findAllByRole('link', { name: 'Personal canary' });
  await act(async () => { finish?.({ hits: [office] }); await Promise.resolve(); });
  expect(screen.queryAllByRole('link', { name: 'Office canary' })).toEqual([]);
  view.unmount();
});
