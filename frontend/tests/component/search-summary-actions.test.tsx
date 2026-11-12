/** Visible source caches are read-only and a result selection never crosses its chosen native root. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { queryOptions, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Summaries from '@/api/summaries';
import type * as Ask from '@/features/ask/open';
import type { NodeRef, SearchHit, SummarySnapshot } from '@/generated/api-types';
import { client } from '@/api/client';
import { AskAboutResults, CachedHitSummary } from '@/features/summaries/SearchSummaries';
import { keyOfHit } from '@/features/search/hits';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
const host = vi.hoisted(() => ({ rows: new Map<string, SummarySnapshot>(), pending: new Map<string, Promise<SummarySnapshot>>(), errors: new Map<string, Error>(), ask: vi.fn<typeof Ask.openAskWithDraft>(), refresh: vi.fn() }));
vi.mock('@/features/ask/open', () => ({ openAskWithDraft: host.ask }));
vi.mock('@/api/summaries', async (original) => {
  const actual = await original<typeof Summaries>();
  return { ...actual, refreshResourceSummary: host.refresh,
    searchSummaryProjectionQuery: (target: { kind: string; id: string }) => queryOptions({ queryKey: ['search-native-summary', actual.summaryReaderKey(), target.kind, target.id], queryFn: () => {
      const error = host.errors.get(target.id);
      if (error !== undefined) return Promise.reject(error);
      const pending = host.pending.get(target.id);
      if (pending !== undefined) return pending;
      const row = host.rows.get(target.id);
      return row === undefined ? Promise.reject(new Error('The source is unavailable')) : Promise.resolve(row);
    }, retry: false }),
  };
});
const root = (id: string, name: string): NodeRef => ({ id, name, kind: 'org', redacted: false });
const hit = (id: string, home: NodeRef): Extract<SearchHit, { kind: 'name' }> => ({ kind: 'name', node: { id, name: id, kind: 'topic', redacted: false }, path: [home], href: `/topic/${id}`, at: '2026-11-08T10:00:00Z', snippet: `Visible snippet ${id}`, matches: [] });
const saved = (id: string, home: string, text: string): SummarySnapshot => ({ root_org_id: home, target: { kind: 'topic', id }, inspecting: false, revision: 1, state: 'fresh', text, producer: null, updated_at: null, route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null });
const sessionBefore = client.session.getState();
const containingText = (text: string): unknown => expect.stringContaining(text);
function CaptureCache({ receive }: { receive: (cache: QueryClient) => void }) {
  const cache = useQueryClient();
  useEffect(() => receive(cache), [cache, receive]);
  return null;
}
afterEach(async () => { cleanup(); host.ask.mockClear(); host.refresh.mockClear(); host.rows.clear(); host.pending.clear(); host.errors.clear(); client.session.setState(sessionBefore, true); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps a failed cached-summary read visible and retries only that free read (${locale})`, async () => {
    await setLocale(locale); const home = root('root-a', 'Work'), one = hit('Readable search hit', home);
    host.errors.set(one.node.id, new Error('Hidden cache, grant and audience canary'));
    show(<CachedHitSummary hit={one} />);
    // EN: Summary read fails → generic inspector line → Try again → stored paragraph, without Refresh or Ask.
    // 中文：摘要读取失败 → 一行通用说明 → 重试 → 已有段落，不刷新摘要，也不提问。
    const reason = await screen.findByText(i18n.t('summaryBox.viewUnavailable'));
    expect(reason.closest('[data-state-line]')).not.toBeNull();
    expect(screen.queryByText('Hidden cache, grant and audience canary')).not.toBeInTheDocument();
    host.errors.delete(one.node.id);
    host.rows.set(one.node.id, saved(one.node.id, home.id, 'Stored paragraph after a free read retry.'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText('Stored paragraph after a free read retry.')).toBeVisible();
    expect(screen.queryByText(i18n.t('summaryBox.viewUnavailable'))).not.toBeInTheDocument();
    expect(host.refresh).not.toHaveBeenCalled(); expect(host.ask).not.toHaveBeenCalled();
  });
  it(`distinguishes a pending projection from a successful empty cache (${locale})`, async () => {
    await setLocale(locale); const home = root('root-a', 'Work'), one = hit('A source without a paragraph', home);
    const pending: { resolve: (value: SummarySnapshot) => void } = { resolve: vi.fn() };
    host.pending.set(one.node.id, new Promise<SummarySnapshot>((resolve) => { pending.resolve = resolve; }));
    show(<CachedHitSummary hit={one} />);
    expect(await screen.findByText(i18n.t('summaryBox.reading'))).toBeVisible();
    expect(screen.queryByText(i18n.t('summaryBox.viewUnavailable'))).not.toBeInTheDocument();
    pending.resolve({ ...saved(one.node.id, home.id, ''), text: null, state: 'empty' });
    await waitFor(() => expect(screen.queryByText(i18n.t('summaryBox.reading'))).not.toBeInTheDocument());
    expect(screen.queryByText(i18n.t('summaryBox.viewUnavailable'))).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
    expect(host.refresh).not.toHaveBeenCalled();
  });
  it(`retries only free source reads after a temporary failure (${locale})`, async () => {
    await setLocale(locale); const home = root('root-a', 'Work'), one = hit('A temporarily unavailable source', home);
    show(<AskAboutResults hits={[one]} roots={new Map([[keyOfHit(one), home]])} slugOfOrg={() => null} />);
    await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
    host.rows.set(one.node.id, saved(one.node.id, home.id, 'Readable after retry.'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') })).not.toHaveAttribute('aria-disabled', 'true'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') }));
    expect(host.ask).toHaveBeenCalledExactlyOnceWith({ root: home.id, text: containingText('Readable after retry.') });
    expect(host.refresh).not.toHaveBeenCalled();
  });
  it(`refuses an old frozen paragraph removed by a current projection (${locale})`, async () => {
    await setLocale(locale); const first = root('root-a', 'Work'), second = root('root-b', 'Other work');
    const one = hit('A visible source', first), two = hit('Another source', second);
    host.rows.set(one.node.id, saved(one.node.id, first.id, 'A paragraph that will be removed.'));
    host.rows.set(two.node.id, saved(two.node.id, second.id, 'Another readable paragraph.'));
    const captured: { cache: QueryClient | null } = { cache: null };
    show(<><CaptureCache receive={(cache) => { captured.cache = cache; }} /><CachedHitSummary hit={one} /><AskAboutResults hits={[one, two]} roots={new Map([[keyOfHit(one), first], [keyOfHit(two), second]])} slugOfOrg={() => null} /></>);
    await screen.findByText('A paragraph that will be removed.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') }));
    const cache = captured.cache;
    if (cache === null) throw new Error('The component query cache is required');
    const removed: SummarySnapshot = { ...saved(one.node.id, first.id, ''), state: 'no_summarizer', text: null };
    host.rows.set(one.node.id, removed);
    act(() => { cache.setQueriesData({ queryKey: ['search-native-summary'], predicate: (query) => query.queryKey.at(-1) === one.node.id }, removed); });
    const action = await screen.findByRole('menuitem', { name: i18n.t('summaryBox.askIn', { name: first.name }) });
    await waitFor(() => expect(action).toHaveAttribute('aria-disabled', 'true'));
    await userEvent.click(action);
    expect(host.ask).not.toHaveBeenCalled(); expect(host.refresh).not.toHaveBeenCalled();
  });
  it(`closes a frozen root menu when the reader changes (${locale})`, async () => {
    await setLocale(locale); const first = root('root-a', 'Work'), second = root('root-b', 'Other work');
    const one = hit('Reader A source', first), two = hit('Reader A other source', second);
    host.rows.set(one.node.id, saved(one.node.id, first.id, 'Reader A paragraph.'));
    host.rows.set(two.node.id, saved(two.node.id, second.id, 'Reader A other paragraph.'));
    show(<><CachedHitSummary hit={one} /><AskAboutResults hits={[one, two]} roots={new Map([[keyOfHit(one), first], [keyOfHit(two), second]])} slugOfOrg={() => null} /></>);
    await screen.findByText('Reader A paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') }));
    await screen.findByRole('menuitem', { name: i18n.t('summaryBox.askIn', { name: first.name }) });
    host.rows.clear();
    act(() => client.session.setState({ userId: 'reader-b', readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
    await waitFor(() => expect(screen.queryByRole('menuitem', { name: i18n.t('summaryBox.askIn', { name: first.name }) })).not.toBeInTheDocument());
    expect(host.ask).not.toHaveBeenCalled(); expect(host.refresh).not.toHaveBeenCalled();
  });
  for (const state of ['unavailable', 'inspection', 'pending'] as const) {
    it(`keeps a ready root askable when another root is ${state} (${locale})`, async () => {
      await setLocale(locale);
      const first = root('root-a', 'Ready work'), second = root('root-b', 'Other work');
      const one = hit('Root A ready', first), two = hit('Root B blocked', second);
      host.rows.set(one.node.id, saved(one.node.id, first.id, 'Only the ready root paragraph.'));
      const pending: { resolve: (value: SummarySnapshot) => void } = { resolve: vi.fn() };
      if (state === 'inspection') host.rows.set(two.node.id, { ...saved(two.node.id, second.id, 'Inspection-only paragraph.'), inspecting: true });
      if (state === 'pending') host.pending.set(two.node.id, new Promise<SummarySnapshot>((resolve) => { pending.resolve = resolve; }));
      show(<><CachedHitSummary hit={one} /><AskAboutResults hits={[one, two]} roots={new Map([[keyOfHit(one), first], [keyOfHit(two), second]])} slugOfOrg={() => null} /></>);
      await screen.findByText('Only the ready root paragraph.');
      await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') }));
      const ready = await screen.findByRole('menuitem', { name: i18n.t('summaryBox.askIn', { name: first.name }) });
      expect(ready).not.toHaveAttribute('aria-disabled', 'true');
      expect(screen.getByRole('menuitem', { name: i18n.t('summaryBox.askIn', { name: second.name }) })).toHaveAttribute('aria-disabled', 'true');
      await userEvent.click(ready);
      expect(host.ask).toHaveBeenCalledExactlyOnceWith({ root: first.id, text: containingText('Only the ready root paragraph.') });
      expect(host.ask.mock.calls[0]?.[0].text).not.toContain('Root B');
      expect(host.refresh).not.toHaveBeenCalled();
      if (state === 'pending') pending.resolve(saved(two.node.id, second.id, 'A later root B paragraph.'));
    });
  }
  it(`waits for the current source read before using even a known-root result (${locale})`, async () => {
    await setLocale(locale); const home = root('root-a', 'Work'), one = hit('A current source', home);
    const source: { resolve: (value: SummarySnapshot) => void } = { resolve: vi.fn() };
    host.pending.set(one.node.id, new Promise<SummarySnapshot>((resolve) => { source.resolve = resolve; }));
    show(<AskAboutResults hits={[one]} roots={new Map([[keyOfHit(one), home]])} slugOfOrg={() => null} />);
    const action = screen.getByRole('button', { name: i18n.t('summaryBox.askResults') });
    expect(action).toHaveAttribute('aria-disabled', 'true'); await userEvent.click(action);
    expect(host.ask).not.toHaveBeenCalled();
    source.resolve(saved(one.node.id, home.id, 'A current readable paragraph.'));
    await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') })).not.toHaveAttribute('aria-disabled', 'true'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') }));
    expect(host.ask).toHaveBeenCalledExactlyOnceWith({ root: home.id, text: containingText('A current readable paragraph.') });
    expect(host.refresh).not.toHaveBeenCalled();
  });
  it(`keeps inspection-only existing paragraphs out of an ordinary Ask seed (${locale})`, async () => {
    await setLocale(locale); const home = root('root-a', 'Work'), one = hit('Inspection-only source', home);
    host.rows.set(one.node.id, { ...saved(one.node.id, home.id, 'Readable inspection paragraph.'), inspecting: true });
    show(<><CachedHitSummary hit={one} /><AskAboutResults hits={[one]} roots={new Map([[keyOfHit(one), home]])} slugOfOrg={() => null} /></>);
    await screen.findByText('Readable inspection paragraph.');
    const action = screen.getByRole('button', { name: i18n.t('summaryBox.askResults') });
    expect(action).toHaveAttribute('aria-disabled', 'true'); await userEvent.click(action);
    expect(host.ask).not.toHaveBeenCalled(); expect(host.refresh).not.toHaveBeenCalled();
  });
  it(`quotes existing visible summaries in native order without requesting generation (${locale})`, async () => {
    await setLocale(locale);
    const home = root('root-a', 'Work'); const one = hit('First visible hit', home), two = hit('Second visible hit', home);
    host.rows.set(one.node.id, saved(one.node.id, home.id, 'First saved paragraph.'));
    host.rows.set(two.node.id, saved(two.node.id, home.id, 'Second saved paragraph.'));
    show(<><CachedHitSummary hit={one} /><CachedHitSummary hit={two} /><AskAboutResults hits={[one, two]} roots={new Map([[keyOfHit(one), home], [keyOfHit(two), home]])} slugOfOrg={() => null} /></>);
    await screen.findByText('First saved paragraph.'); await screen.findByText('Second saved paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') }));
    const opening = host.ask.mock.calls[0]?.[0] as { root: string; text: string } | undefined;
    expect(opening?.root).toBe(home.id);
    expect(opening?.text.indexOf('First saved paragraph.')).toBeLessThan(opening?.text.indexOf('Second saved paragraph.') ?? -1);
    expect(opening?.text).toContain('/topic/First visible hit');
    expect(host.refresh).not.toHaveBeenCalled();
  });
  it(`asks about one explicitly chosen visible root group (${locale})`, async () => {
    await setLocale(locale);
    const first = root('root-a', 'Work'), second = root('root-b', 'Other work');
    const one = hit('Root A visible', first), two = hit('Root B visible', second);
    host.rows.set(one.node.id, saved(one.node.id, first.id, 'Root A paragraph.'));
    host.rows.set(two.node.id, saved(two.node.id, second.id, 'Root B paragraph.'));
    show(<AskAboutResults hits={[one, two]} roots={new Map([[keyOfHit(one), first], [keyOfHit(two), second]])} slugOfOrg={() => null} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.askResults') }));
    await userEvent.click(await screen.findByRole('menuitem', { name: i18n.t('summaryBox.askIn', { name: second.name }) }));
    expect(host.ask).toHaveBeenCalledTimes(1);
    expect(host.ask.mock.calls[0]?.[0]).toEqual(expect.objectContaining({ root: second.id }));
    expect(host.ask.mock.calls[0]?.[0].text).toContain('Root B');
    expect(host.ask.mock.calls[0]?.[0].text).not.toContain('Root A');
    expect(host.refresh).not.toHaveBeenCalled();
  });
}
