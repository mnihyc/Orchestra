/** The Links section: its tabs and counts, the lines around each mention, locked targets, and what a hidden tab does. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { LinksPanel } from '@/features/links/Links';
import type * as LinksApi from '@/api/links';
import type { DocumentLinks, LinkGraph, LinkedThing, RelatedItems } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

interface Listing { items: LinkedThing[]; next_cursor: string | null }
const calls = vi.hoisted(() => ({
  preview: vi.fn<() => Promise<unknown>>(),
  tabs: vi.fn<(id: string) => Promise<DocumentLinks>>(),
  references: vi.fn<(id: string, direction: string, cursor: string | null) => Promise<Listing>>(),
  mentions: vi.fn<(id: string, of: string, cursor: string) => Promise<Listing>>(),
  related: vi.fn<(id: string, seq: number, cursor: string | null) => Promise<RelatedItems>>(),
  graph: vi.fn<(id: string) => Promise<LinkGraph>>(),
  go: vi.fn<(path: string) => void>(),
}));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<Record<string, unknown>>(), useRouter: () => ({ history: { push: calls.go } }) }));
vi.mock('@/api/references', () => ({ referencePreviewQuery: (reference: unknown) => ({ queryKey: ['links-preview-test', reference], queryFn: calls.preview, staleTime: Infinity, retry: false }) }));
vi.mock('@/api/links', async (original) => {
  const real = await original<typeof LinksApi>();
  const { infiniteQueryOptions, queryOptions } = await import('@tanstack/react-query');
  return {
    ...real,
    documentLinksQuery: (id: string) => queryOptions({ queryKey: ['t-links', id], queryFn: () => calls.tabs(id), staleTime: Infinity, retry: false }),
    referencesQuery: (id: string, direction: string) => infiniteQueryOptions({
      queryKey: ['t-references', id, direction], initialPageParam: null as string | null, staleTime: Infinity, retry: false,
      queryFn: ({ pageParam }) => calls.references(id, direction, pageParam), getNextPageParam: (page: { next_cursor: string | null }) => page.next_cursor,
    }),
    mentionsQuery: (id: string, of: string, first: string) => infiniteQueryOptions({
      queryKey: ['t-mentions', id, of], initialPageParam: first, staleTime: Infinity, retry: false,
      queryFn: ({ pageParam }) => calls.mentions(id, of, pageParam), getNextPageParam: (page: { next_cursor: string | null }) => page.next_cursor,
    }),
    relatedPagesQuery: (id: string, seq: number) => infiniteQueryOptions({
      queryKey: ['t-related', id, seq], initialPageParam: null as string | null, staleTime: Infinity, retry: false,
      queryFn: ({ pageParam }) => calls.related(id, seq, pageParam), getNextPageParam: (page: { next_cursor?: string | null }) => page.next_cursor ?? null,
    }),
    graphQuery: (id: string) => queryOptions({ queryKey: ['t-graph', id], queryFn: () => calls.graph(id), staleTime: Infinity, retry: false }),
  };
});

const all: DocumentLinks = { shown: ['referenced_by', 'references', 'related', 'graph'], own: null, inherited: ['referenced_by', 'references', 'related', 'graph'], locked: false, writable: true };
const source = (over: Partial<LinkedThing> = {}): LinkedThing => ({
  kind: 'document', id: '019a0000-0000-7000-8000-000000000001', name: 'Notes', label: null, readable: true, context: [], mentions: 2, mentions_more: false, next_mentions: null,
  occurrences: [
    { anchor: { type: 'block', block_id: 'block-1' }, line: { before: 'Decision: see ', reference: 'Plan', after: ' for the dates.' }, by: null, at: '2026-10-28T10:00:00.000Z' },
    { anchor: { type: 'comment', comment_id: 'c1', discussion_id: 'd1', block_id: 'block-2' }, line: { before: 'Was ', reference: 'Plan', after: ' agreed?' }, by: null, at: '2026-10-28T11:00:00.000Z' },
  ],
  ...over,
});
const locked: LinkedThing = { kind: 'document', id: '019a0000-0000-7000-8000-000000000009', name: null, label: 'Budget talks', readable: false, context: [], mentions: 1, mentions_more: false, occurrences: [], next_mentions: null };
const page = (items: LinkedThing[], next: string | null = null) => ({ items, next_cursor: next });

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  calls.tabs.mockResolvedValue(all);
  calls.preview.mockResolvedValue({ kind: 'document', id: locked.id, title: '', node_kind: null, place: null, description: '', updated_at: null, updated_by: null, part: null, unavailable: { deleted: false, object: null, permission: 'doc.view', requestable: false, pending: null } });
  calls.references.mockImplementation((_id: string, direction: string) => Promise.resolve(direction === 'in' ? page([source()]) : page([locked])));
  calls.related.mockResolvedValue({ seq: 1, items: [], meaning: 'ready', meaning_reason: null });
  calls.graph.mockResolvedValue({ center: 'x', nodes: [], edges: [], truncated: false, meaning: 'off' });
});
afterEach(async () => { cleanup(); vi.unstubAllGlobals(); await setLocale('en'); });

it('shows the four tabs, each of the first two with its count', async () => {
  show(<LinksPanel documentId="plan" seq={1} />);
  await screen.findByRole('list', { name: 'Referenced by' });
  const tabs = screen.getByRole('tablist', { name: 'Links' });
  for (const name of ['Referenced by', 'References', 'Related', 'Graph']) expect(within(tabs).getByRole('tab', { name })).toBeVisible();
  await waitFor(() => expect(within(screen.getByRole('tab', { name: 'References' })).getByText('1')).toBeVisible());
  expect(within(screen.getByRole('tab', { name: 'Referenced by' })).getByText('1')).toBeVisible();
});

it('opens a source to the line around each mention with the reference in bold, and a click goes to that place', async () => {
  const user = userEvent.setup();
  show(<LinksPanel documentId="plan" seq={1} />);
  const row = await screen.findByText('2 mentions');
  expect(row).toHaveClass('text-text-muted');
  expect(screen.queryByText('Decision: see')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Show mentions in Notes' }));
  const mentions = await screen.findAllByRole('button', { name: 'Open Notes at this mention' });
  expect(mentions).toHaveLength(2);
  expect(within(mentions[0]!).getByText('Plan').tagName).toBe('STRONG');
  expect(mentions[0]).toHaveTextContent('Decision: see Plan for the dates.');
  await user.click(mentions[1]!);
  expect(calls.go).toHaveBeenCalledWith('/doc/019a0000-0000-7000-8000-000000000001?block=block-2');
  await user.click(screen.getByRole('button', { name: 'Hide mentions in Notes' }));
  expect(screen.queryByRole('button', { name: 'Open Notes at this mention' })).not.toBeInTheDocument();
});

it('goes on with a source’s own mentions from the cursor its row carries', async () => {
  const user = userEvent.setup();
  calls.references.mockImplementation((_id: string, direction: string) => Promise.resolve(direction === 'in' ? page([source({ mentions: 3, next_mentions: 'more-1' })]) : page([])));
  calls.mentions.mockResolvedValue(page([source({ next_mentions: null, occurrences: [{ anchor: { type: 'message', message_id: 'm9' }, line: { before: '', reference: 'Plan', after: ' later' }, by: null, at: '2026-10-28T12:00:00.000Z' }] })]));
  show(<LinksPanel documentId="plan" seq={1} />);
  await user.click(await screen.findByRole('button', { name: 'Show mentions in Notes' }));
  expect(calls.mentions).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Show more' }));
  await waitFor(() => expect(screen.getAllByRole('button', { name: 'Open Notes at this mention' })).toHaveLength(3));
  expect(calls.mentions).toHaveBeenCalledExactlyOnceWith('plan', 'document:019a0000-0000-7000-8000-000000000001', 'more-1');
  expect(screen.queryByRole('button', { name: 'Show more' })).not.toBeInTheDocument();
});

it('lists what the document refers to with a locked target by the words its author wrote', async () => {
  const user = userEvent.setup();
  show(<LinksPanel documentId="plan" seq={1} />);
  await user.click(await screen.findByRole('tab', { name: 'References' }));
  const list = await screen.findByRole('list', { name: 'References' });
  expect(within(list).getByRole('button', { name: 'Budget talks' })).toHaveAttribute('data-reference-state', 'no-access');
  expect(list).not.toHaveTextContent('mention');
});

it('says so when nothing refers to the document or is referred to', async () => {
  const user = userEvent.setup();
  calls.references.mockResolvedValue(page([]));
  show(<LinksPanel documentId="plan" seq={1} />);
  expect(await screen.findByText('Nothing refers to this document yet')).toBeVisible();
  await user.click(screen.getByRole('tab', { name: 'References' }));
  expect(await screen.findByText('This document refers to nothing yet')).toBeVisible();
});

it('goes on past a page that holds nothing the reader may read, and says "or more" for a count that stops', async () => {
  const user = userEvent.setup();
  calls.references.mockImplementation((_id: string, direction: string, cursor: string | null) => Promise.resolve(direction !== 'in' ? page([]) : cursor === null
    ? { items: [], next_cursor: 'later' }
    : page([source({ mentions: 100, mentions_more: true })])));
  show(<LinksPanel documentId="plan" seq={1} />);
  expect(screen.queryByText('Nothing refers to this document yet')).not.toBeInTheDocument();
  await user.click(await screen.findByRole('button', { name: 'Show more' }));
  expect(await screen.findByText('100 or more mentions')).toBeVisible();
  expect(calls.references).toHaveBeenCalledWith('plan', 'in', 'later');
});

it('shares a page request when Show more and its scroll observer fire together', async () => {
  const watchers: ((entries: { isIntersecting: boolean }[]) => void)[] = [];
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: (entries: { isIntersecting: boolean }[]) => void) { watchers.push(callback); }
    observe() { /* the test supplies visibility */ }
    disconnect() { /* no resources are held */ }
  });
  const rows = Array.from({ length: 29 }, (_, index) => source({ id: `source-${index}`, name: `Notes ${index}`, occurrences: [] }));
  let finish: ((value: Listing) => void) | undefined;
  const next = new Promise<Listing>((resolve) => { finish = resolve; });
  calls.references.mockImplementation((_id, direction, cursor) => direction !== 'in'
    ? Promise.resolve(page([])) : cursor === null ? Promise.resolve(page(rows.slice(0, 20), 'page-2')) : next);
  show(<LinksPanel documentId="plan" seq={1} />);
  const list = await screen.findByRole('list', { name: 'Referenced by' });
  expect(within(list).getAllByRole('link')).toHaveLength(20);
  const more = screen.getByRole('button', { name: 'Show more' });
  const intersect = watchers.at(-1);
  expect(intersect).toBeDefined();
  await act(async () => {
    fireEvent.click(more);
    intersect?.([{ isIntersecting: true }]);
    await Promise.resolve();
  });
  expect(calls.references.mock.calls.filter(([, direction, cursor]) => direction === 'in' && cursor === 'page-2')).toHaveLength(1);
  await act(async () => { finish?.(page(rows.slice(20))); await next; });
  await waitFor(() => expect(within(list).getAllByRole('link')).toHaveLength(29));
  expect(calls.references.mock.calls.filter(([, direction, cursor]) => direction === 'in' && cursor === 'page-2')).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Show more' })).not.toBeInTheDocument();
});

it('draws only the tabs the document shows, and nothing when it shows none', async () => {
  calls.tabs.mockResolvedValue({ ...all, shown: ['related', 'graph'] });
  const view = show(<LinksPanel documentId="plan" seq={1} />);
  await screen.findByRole('tablist', { name: 'Links' });
  expect(screen.queryByRole('tab', { name: 'Referenced by' })).not.toBeInTheDocument();
  expect(screen.getByRole('tab', { name: 'Related' })).toHaveAttribute('data-state', 'active');
  expect(calls.references).not.toHaveBeenCalled();
  view.unmount();
  calls.tabs.mockResolvedValue({ ...all, shown: [] });
  const none = show(<LinksPanel documentId="plan" seq={1} />);
  await waitFor(() => expect(calls.tabs).toHaveBeenCalledTimes(2));
  expect(none.container.querySelector('[data-links-section]')).toBeNull();
});

it('keeps a failure local with a retry', async () => {
  const user = userEvent.setup();
  calls.references.mockRejectedValueOnce(new Error('failed')).mockImplementation(() => Promise.resolve(page([source()])));
  show(<LinksPanel documentId="plan" seq={1} />);
  await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByRole('list', { name: 'Referenced by' })).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('names its tabs in %s', async (locale) => {
  await setLocale(locale);
  show(<LinksPanel documentId="plan" seq={1} />);
  await screen.findByRole('tablist');
  for (const tab of ['referenced_by', 'references', 'related', 'graph'] as const) expect(screen.getByRole('tab', { name: i18n.t(`links.tabs.${tab}`) })).toBeVisible();
});
