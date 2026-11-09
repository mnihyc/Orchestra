/** Common web actions are titled from normalized facts, with safe targets and legacy search rows. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { describeStep } from '@/features/timeline/titles';
import { summarize } from '@/features/timeline/Steps';
import { WebSearchDisplay } from '@/features/timeline/tools/WebSearch';
import { webAction } from '@/features/timeline/tools/webAction';
import { i18n, setLocale } from '@/i18n';
import { toolCall } from '../doubles/thread-rows';
import { show } from './support';

const URL = 'https://docs.example.invalid/guide';
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`uses the reported operation and never labels a page open as an empty search (${locale})`, async () => {
    await setLocale(locale);
    const tool = { kind: 'web_search' as const, query: '', results: [], action: { type: 'open_page' as const, url: URL } };
    const item = toolCall('open', tool, { title: 'Native title' });
    if (item.kind !== 'tool_call') throw new Error('The fixture is a tool call');
    expect(describeStep(item, i18n.t).text).toBe(locale === 'en' ? 'Open page: docs.example.invalid/guide' : '打开网页：docs.example.invalid/guide');
    expect(summarize([item])).toMatchObject({ searches: 0, fetches: 1 });
    show(<WebSearchDisplay item={item} />);
    expect(screen.getByRole('link')).toHaveAttribute('href', URL);
    expect(screen.getByRole('link')).toHaveAttribute('rel', 'noopener noreferrer');
    expect(screen.queryByText(i18n.t('timeline.web.noResults'))).not.toBeInTheDocument();
  });
  it(`keeps a page pattern and unreported target distinct from search results (${locale})`, async () => {
    await setLocale(locale);
    const tool = { kind: 'web_search' as const, query: '', results: [], action: { type: 'find_in_page' as const, url: null, pattern: 'harvest' } };
    const item = toolCall('find', tool, { title: 'Native title' });
    if (item.kind !== 'tool_call') throw new Error('The fixture is a tool call');
    expect(describeStep(item, i18n.t).text).toBe(locale === 'en' ? 'Find in page: harvest' : '页内查找：harvest');
    show(<WebSearchDisplay item={item} />);
    expect(screen.getByText(locale === 'en' ? 'No page address was reported' : '未报告网页地址')).toBeVisible();
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('timeline.web.noResults'))).not.toBeInTheDocument();
  });
  it(`keeps an unsafe reported target inert and a legacy search readable (${locale})`, async () => {
    await setLocale(locale);
    const tool = { kind: 'web_search' as const, query: '', results: [], action: { type: 'open_page' as const, url: 'javascript:alert(1)' } };
    const item = toolCall('unsafe', tool, { title: 'Native title' });
    if (item.kind !== 'tool_call') throw new Error('The fixture is a tool call');
    const view = show(<WebSearchDisplay item={item} />);
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    view.unmount();
    const old = toolCall('old', { kind: 'web_search', query: 'orchards', results: [] }, { title: 'Legacy title' });
    if (old.kind !== 'tool_call') throw new Error('The fixture is a tool call');
    expect(describeStep(old, i18n.t).text).toBe(i18n.t('timeline.step.webSearch', { query: 'orchards' }));
    show(<WebSearchDisplay item={old} />);
    expect(screen.getByText(i18n.t('timeline.web.noResults'))).toBeVisible();
  });
}

it('leaves future and malformed action kinds generic while an absent action stays a legacy search', () => {
  expect(webAction({ action: { type: 'future_action' } }).type).toBe('other');
  expect(webAction({ action: 'open_page' }).type).toBe('other');
  expect(webAction({}).type).toBe('search');
});
