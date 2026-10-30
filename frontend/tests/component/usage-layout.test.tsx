/** All Usage views use the same keyboard tabs and keep the reader's span and filters. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { useState, type AnchorHTMLAttributes, type ReactNode } from 'react';
import type * as RouterModule from '@tanstack/react-router';
import type { Node, UsageAnalytics, UsageDimension } from '@/generated/api-types';
import type { EventParams, UsageParams } from '@/api/usage';
import type * as UsageApi from '@/api/usage';
import { Report } from '@/features/usage/Report';
import { patchSearch, type UsageSearch } from '@/features/usage/search';
import { show } from './support';
import { requestRow } from '../doubles/request-row';
import { me, org, userRef } from '../doubles/fixtures';
import { i18n, setLocale } from '@/i18n';

const usageScene = vi.hoisted(() => ({ active: false }));
const usageRead = vi.hoisted(() => vi.fn<(params: UsageParams) => void>());
const historyRead = vi.hoisted(() => vi.fn<(params: EventParams) => void>());
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof RouterModule>(),
  Link: ({ to, params, search, children, ...anchor }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; params?: Record<string, string>; search?: Record<string, string | number | boolean | undefined>; children?: ReactNode }) => {
    const path = to.replace(/\$(\w+)/g, (_match: string, name: string) => params?.[name] ?? '');
    const query = new URLSearchParams(Object.entries(search ?? {}).flatMap(([key, value]): [string, string][] => value === undefined ? [] : [[key, String(value)]])).toString();
    return <a {...anchor} href={`${path}${query === '' ? '' : `?${query}`}`}>{children}</a>;
  },
}));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: () => undefined }));
vi.mock('@/features/usage/Export', () => ({ Export: () => null }));
// Chart rendering has its own component tests; these cases exercise the report's controlled inputs.
vi.mock('@/features/usage/Chart', () => ({ UsageChart: () => null }));
vi.mock('@/api/nodes', () => ({ nodeQuery: (id: string) => ({ queryKey: ['layout-node', id], queryFn: (): Promise<Node> => Promise.resolve(org(id, 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')))) }), orgsQuery: {queryKey:['orgs'],queryFn:()=>Promise.resolve([])}, fetchTree:()=>Promise.resolve({nodes:[]}) }));
vi.mock('@/api/resources', () => ({ providerQuery: () => ({ queryKey: ['layout-provider'], queryFn: () => Promise.resolve(null) }) }));
vi.mock('@/realtime/live', () => ({ socket: { subscribe: () => ({ unsubscribe: () => undefined }) } }));
vi.mock('@/api/usage', async (importOriginal) => {
  const original = await importOriginal<typeof UsageApi>();
  return {
  ...original,
  eventsQuery: (params: EventParams) => ({ ...original.eventsQuery(params), queryKey: ['layout-records', params], initialPageParam: null as string | null, queryFn: () => {
    historyRead(params);
    return Promise.resolve({ items: params.filters.model === undefined ? [] : [requestRow({ model: params.filters.model, model_display: 'Swift', at: new Date().toISOString() })], next_cursor: null });
  }, getNextPageParam: () => null }),
  usageQuery: (params: UsageParams) => ({ ...original.usageQuery(params), queryKey: ['layout-usage', params], queryFn: (): Promise<UsageAnalytics> => { usageRead(params);
    const totals = { turns: usageScene.active ? 1 : 0, tokens: { input_tokens: usageScene.active ? 10 : 0, cached_input_tokens: 0, output_tokens: usageScene.active ? 5 : 0, reasoning_tokens: 0, total_tokens: usageScene.active ? 15 : 0 }, credits: usageScene.active ? '1' : '0', estimated_credits: '0', unrated_tokens: 0 };
    return Promise.resolve({
    scope: params.scope, from: params.from, to: params.to, bucket: params.bucket ?? 'day', depth: 'own', me: null,
    totals,
    series: usageScene.active ? [{ at: `${params.from}T00:00:00Z`, figures: totals, by: [] }] : [], breakdowns: params.breakdowns.map((by) => ({ by, more: 0, rows: [] })),
  }); } }),
  fetchEventsPage: () => Promise.resolve({ items: [], next_cursor: null }),
};
});

afterEach(async () => { cleanup(); usageScene.active = false; document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });
function GlobalNavigation() { return <nav aria-label="Project navigation">Acme</nav>; }

it.each(['usage', 'performance'] as const)('locates every scoped %s split locally while keeping descriptions and None', async (view) => {
  usageRead.mockClear(); historyRead.mockClear(); usageScene.active = true;
  const user = userEvent.setup({ delay: null }), onSearch = vi.fn();
  const scope = { kind: 'node', id: '00000000-0000-4000-8000-000000000001' } as const;
  function ControlledReport() {
    const [search, setSearch] = useState<UsageSearch>({ view: view === 'usage' ? undefined : view, range: 'last_week' });
    return <Report embedded scope={scope} search={search} onSearch={(patch) => {
      onSearch(patch);
      setSearch((before) => patchSearch(before, patch));
    }} />;
  }
  show(<ControlledReport />);
  const input = await screen.findByRole('combobox', { name: i18n.t('usage.chart.splitBy') });
  const expected: (UsageDimension | 'none')[] = ['thread', 'person', 'model', 'provider', 'agent', 'environment', 'org', 'project', 'tier', 'effort', 'program',
    ...(view === 'performance' ? ['topic' as const] : []), 'none'];
  let reads = usageRead.mock.calls.length;
  expect(reads).toBeGreaterThan(0);
  // Usage also mounts its ordinary Records table; local choice typing must add no reads.
  await waitFor(() => expect(historyRead).toHaveBeenCalledTimes(view === 'usage' ? 1 : 0));
  const historyReads = historyRead.mock.calls.slice();
  await user.click(input);
  expect(expected.length).toBeGreaterThan(10);
  const options = within(screen.getByRole('listbox')).getAllByRole('option');
  expect(options).toHaveLength(expected.length);
  for (const [index, split] of expected.entries()) {
    expect(options[index]).toHaveAccessibleName(i18n.t(`usagePerf.choices.splits.${split}.name`));
    expect(options[index]).toHaveAccessibleDescription(i18n.t(`usagePerf.choices.splits.${split}.description`));
  }
  await user.type(input, 'program', { skipClick: true });
  expect(onSearch).not.toHaveBeenCalled(); expect(usageRead).toHaveBeenCalledTimes(reads); expect(historyRead.mock.calls).toEqual(historyReads);
  await user.keyboard('{Enter}');
  expect(onSearch).toHaveBeenCalledExactlyOnceWith({ series: 'program' });
  // Applying a committed split reads that series, just as the actual controlled page does.
  reads += 1;
  await waitFor(() => expect(usageRead).toHaveBeenCalledTimes(reads));
  expect(usageRead).toHaveBeenLastCalledWith(expect.objectContaining({ seriesBy: 'program' }));
  const nextInput = await screen.findByRole('combobox', { name: i18n.t('usage.chart.splitBy') });
  expect(nextInput).toHaveValue(i18n.t('usagePerf.choices.splits.program.name'));
  await user.click(nextInput);
  const nextOptions=within(screen.getByRole('listbox'));
  await user.type(nextInput, i18n.t('usagePerf.choices.splits.none.name'), { skipClick: true });
  expect(usageRead).toHaveBeenCalledTimes(reads);
  expect(nextOptions.getByRole('option', { name: i18n.t('usagePerf.choices.splits.none.name') })).toBeVisible();
  expect(nextInput).toHaveFocus();
  expect(nextInput).toHaveValue(i18n.t('usagePerf.choices.splits.none.name'));
  // Hover changes the active row. Resolve its live element again for the subsequent click.
  await user.hover(nextOptions.getByRole('option', { name: i18n.t('usagePerf.choices.splits.none.name') }));
  await user.click(nextOptions.getByRole('option', { name: i18n.t('usagePerf.choices.splits.none.name') }));
  expect(onSearch).toHaveBeenLastCalledWith({ series: 'none' });
  reads += 1;
  await waitFor(() => expect(usageRead).toHaveBeenCalledTimes(reads));
  expect(usageRead).toHaveBeenLastCalledWith(expect.objectContaining({ seriesBy: undefined }));
  expect(usageRead).toHaveBeenCalledTimes(reads);
  expect(historyRead.mock.calls).toEqual(historyReads);
  // The independent measure and bucket controls retain their existing finite Select workflow.
  expect(screen.getByRole('combobox', { name: i18n.t('usage.chart.per') }).tagName).toBe('BUTTON');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps its local view choices while global navigation opens and closes at ${locale}/${theme}`, async () => {
  await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
  const search = { range: 'last_week', model: 'Swift' } as const;
  const view = show(<><GlobalNavigation /><Report scope={{ kind: 'me' }} search={search} /></>);
  expect(screen.getByRole('navigation', { name: 'Project navigation' })).toBeVisible();
  const originalTabs = screen.getByRole('tablist', { name: i18n.t('usagePerf.views') });
  expect(originalTabs).toBeVisible();
  expect(screen.getByRole('tabpanel', { name: i18n.t('usagePerf.viewNames.usage') })).toBeInTheDocument();
  expect(screen.getByRole('radio', { name: i18n.t('usagePerf.periodNames.last_week') })).toHaveAttribute('aria-checked', 'true');
  view.rerender(<>{null}<Report scope={{ kind: 'me' }} search={search} /></>);
  const tabs = screen.getByRole('tablist', { name: i18n.t('usagePerf.views') });
  expect(tabs).toBe(originalTabs);
  expect(within(tabs).getAllByRole('tab')).toHaveLength(3);
  expect(screen.getByRole('radio', { name: i18n.t('usagePerf.periodNames.last_week') })).toHaveAttribute('aria-checked', 'true');
});

it('keeps compact embedded view segments beside the period without focusing them on load', async () => {
  const user = userEvent.setup();
  show(<Report embedded scope={{ kind: 'me' }} />);
  const tabs = screen.getByRole('radiogroup', { name: 'Usage views' });
  expect(within(tabs).getAllByRole('radio').map((tab) => tab.textContent)).toEqual(['Consumption', 'Performance', 'Requests']);
  expect(screen.queryByRole('tablist',{name:'Usage views'})).not.toBeInTheDocument();
  expect(within(tabs).getByRole('radio',{name:'Consumption'})).not.toHaveFocus();
  expect(tabs).not.toHaveTextContent('Count turns, tokens and credits.');
  await user.click(screen.getByRole('radio', { name: '7 days' }));
  await user.click(within(tabs).getByRole('radio', { name: 'Performance' }));
  await screen.findByRole('list', { name: 'Performance figures' });
  expect(screen.getByRole('radio', { name: '7 days' })).toHaveAttribute('aria-checked', 'true');
  expect(within(tabs).getByRole('radio', { name: 'Performance' })).toHaveFocus();
  await user.keyboard('{ArrowRight}');
  expect(within(tabs).getByRole('radio', { name: 'Requests' })).toHaveAttribute('aria-checked', 'true');
  expect(screen.getByRole('heading', {name:'Requests',level:2})).toBeInTheDocument();
  await waitFor(()=>expect(within(tabs).getByRole('radio', { name: 'Requests' })).toHaveFocus());
  expect(screen.getByRole('radio', { name: '7 days' })).toHaveAttribute('aria-checked', 'true');
  expect(screen.getAllByRole('button', { name: 'Help: Speed, first output and errors' })).toHaveLength(1);
});

it('removes an active filter using a named ordinary action in every view', async () => {
  historyRead.mockClear();
  const onSearch = vi.fn();
  show(<Report embedded scope={{ kind: 'me' }} search={{ view: 'requests', model: 'model-route' }} onSearch={onSearch} />);
  const filters = screen.getByRole('group', { name: 'What the screen is narrowed to' });
  const request = await screen.findByRole('link', { name: 'Repair redirects' });
  expect(request).toHaveAttribute('href', '/thread/00000000-0000-4000-8000-000000000104?turn=7');
  const remove = await within(filters).findByRole('button', { name: 'Remove Model filter: Swift' });
  expect(remove).toHaveTextContent('Model: Swift');
  expect(filters).not.toHaveTextContent('model-route');
  expect(historyRead).toHaveBeenCalledTimes(1);
  expect(historyRead).toHaveBeenCalledWith(expect.objectContaining({ filters: { model: 'model-route' }, includeRunning: true, requestsOnly: true }));
  await userEvent.setup().click(remove);
  expect(onSearch).toHaveBeenCalledExactlyOnceWith({ model: undefined });
});

it('keeps its one guide in the shared header and says the day zone once', () => {
  show(<Report scope={{ kind: 'me' }} />);
  const guide = screen.getByRole('button', { name: 'Speed, first output and errors' });
  const header = guide.closest('header');
  if (header === null) throw new Error('the guide belongs to the page header');
  expect(within(header).getByRole('heading', { name: 'Usage', level: 1 })).toBeInTheDocument();
  expect(screen.getByRole('combobox',{name:'Scope'}).closest('[data-filter-bar]')).not.toBeNull();
  expect(screen.getByRole('radio',{name:'Last week'}).closest('[data-filter-bar]')).not.toBeNull();
  expect(screen.getAllByText('Days use UTC')).toHaveLength(1);
  expect(screen.queryByText('What you used yourself, everywhere, in full.')).not.toBeInTheDocument();
  expect(screen.queryByText(/by day · UTC/)).not.toBeInTheDocument();
});
