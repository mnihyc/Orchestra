/** Help offers readable articles, explains the real screen and recovers from an absent address. */
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider, useParams } from '@tanstack/react-router';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { client } from '@/api/client';
import { Articles, HelpContent } from '@/features/help/HelpView';
import { useHelpState } from '@/features/help/state';
import { articleInLanguage, makeGuide, type Guide } from '@/help/guide';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const scene = vi.hoisted(() => ({ guide: undefined as Guide | undefined }));
vi.mock('@/help/localized', () => ({ useLocalizedGuide: () => scene.guide }));

const session = client.session.getState();
const help = useHelpState.getState();
beforeEach(() => {
  client.session.setState({ userId: null, status: 'signed_out' });
  useHelpState.setState({ article: null, heading: null, query: '', settingContext: null, guideContext: null });
  scene.guide = makeGuide({
    'readable.md': '---\nscreens: /resources\n---\n# Resource guide\n\nChoose an authorized resource to see its details.',
    'draft.md': '---\nscreens: /resources\n---\n# Draft guide\n\nPlaceholder guide.\n\nPlanned coverage: resources.',
  });
});
afterEach(async () => {
  cleanup(); client.session.setState(session); useHelpState.setState(help);
  document.documentElement.removeAttribute('data-theme'); await setLocale('en');
});

function ArticlePage() {
  const { article } = useParams({ strict: false });
  return <HelpContent page article={article ?? null} />;
}

function draw(address: string) {
  const root = createRootRoute();
  const index = createRoute({ getParentRoute: () => root, path: '/help', component: () => <HelpContent page /> });
  const article = createRoute({ getParentRoute: () => root, path: '/help/$article', component: ArticlePage });
  const provider = createRoute({ getParentRoute: () => root, path: '/resources/providers/$id', component: () => <HelpContent /> });
  const router = createRouter({ routeTree: root.addChildren([index, article, provider]), history: createMemoryHistory({ initialEntries: [address] }) });
  show(<RouterProvider router={router} />);
  return router;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`groups the catalogue with each first line and keeps every article's own destination at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const source = makeGuide({
      'start.md': '---\ngroup: getting-started\n---\n# Begin here\n\nChoose **your project** and `main.ts`.',
      'access.md': '---\ngroup: access\n---\n# Share work\n\nChoose who may view your work.',
      'provider.md': '---\ngroup: providers\n---\n# Choose a provider\n\nUse the model you can access.',
      'builder.md': '---\nsection: build-your-own\n---\n# Build an adapter\n\nWrite an adapter.',
    }, {}, { 'zh-CN': {
      'start.md': '# 从这里开始\n\n选择**你的项目**和 `main.ts`。',
      'access.md': '# 分享工作\n\n选择谁可以查看你的工作。',
      'provider.md': '# 选择提供方\n\n使用你有权访问的模型。',
    } });
    const guide = { articles: source.articles.map((article) => articleInLanguage(article, locale)) };
    const selected = vi.fn<(id: string) => void>();
    show(<Articles guide={guide} onArticle={selected} />);
    for (const group of ['getting-started', 'access', 'providers'] as const) expect(screen.getByRole('heading', { name: i18n.t(`help.articleGroups.${group}`) })).toBeVisible();
    expect(screen.getAllByRole('button')).toHaveLength(3);
    expect(screen.queryByRole('button', { name: 'Build an adapter' })).not.toBeInTheDocument();
    const first = screen.getByRole('button', { name: locale === 'en' ? 'Begin here' : '从这里开始' });
    expect(first).toHaveAccessibleDescription(locale === 'en' ? 'Choose your project and main.ts.' : '选择你的项目和 main.ts。');
    await userEvent.click(first); expect(selected).toHaveBeenCalledExactlyOnceWith('start');
  });
  it(`keeps every matching help article reachable beyond thirty results at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const english: Record<string, string> = {}, chinese: Record<string, string> = {};
    // Thirty-one is the smallest corpus that exposes the former silent thirty-result cut.
    for (let index = 1; index <= 31; index += 1) {
      const number = String(index).padStart(2, '0'), file = `quartz-${number}.md`;
      english[file] = `---\nkeywords: quartz\n---\n# Quartz entry ${number}\n\nRead this public help entry.`;
      chinese[file] = `---\nkeywords: quartz\n---\n# 石英条目 ${number}\n\n阅读这篇公开的帮助条目。`;
    }
    const source = makeGuide(english, {}, { 'zh-CN': chinese });
    scene.guide = { articles: source.articles.map((article) => articleInLanguage(article, locale)) };
    const router = draw('/help');
    const input = await screen.findByRole('searchbox', { name: i18n.t('help.search.label') });
    await userEvent.type(input, 'quartz');
    const matches = screen.getAllByRole('button', { name: locale === 'en' ? /^Quartz entry / : /^石英条目 / });
    expect(matches).toHaveLength(31);
    const title = locale === 'en' ? 'Quartz entry 31' : '石英条目 31';
    await userEvent.click(screen.getByRole('button', { name: title }));
    expect(await screen.findByRole('region', { name: title })).toBeVisible();
    expect(router.state.location.pathname).toBe('/help/quartz-31');
  });
  for (const surface of ['page', 'view'] as const) it(`searches from an open article and opens the chosen result in the ${surface} at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const source = makeGuide({
      'first.md': '# First guide\n\nBegin with this public guide.',
      'second.md': '---\nkeywords: quartz\n---\n# Quartz next\n\nRead the next public instructions.',
    }, {}, { 'zh-CN': {
      'first.md': '# 第一篇指南\n\n从这篇公开指南开始。',
      'second.md': '---\nkeywords: quartz\n---\n# 石英下一篇\n\n阅读下一篇公开说明。',
    } });
    scene.guide = { articles: source.articles.map((article) => articleInLanguage(article, locale)) };
    const first = locale === 'en' ? 'First guide' : '第一篇指南', second = locale === 'en' ? 'Quartz next' : '石英下一篇';
    const router = surface === 'page' ? draw('/help/first') : null;
    if (surface === 'view') { useHelpState.setState({ article: 'first', query: '' }); show(<HelpContent />); }
    expect(await screen.findByRole('region', { name: first })).toBeVisible();
    await userEvent.type(screen.getByRole('searchbox', { name: i18n.t('help.search.label') }), 'quartz');
    await userEvent.click(await screen.findByRole('button', { name: second }));
    expect(await screen.findByRole('region', { name: second })).toBeVisible();
    expect(screen.getByRole('searchbox', { name: i18n.t('help.search.label') })).toHaveValue('');
    if (router !== null) expect(router.state.location.pathname).toBe('/help/second');
  });
  it(`describes a detail screen and opens its readable article at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    draw('/resources/providers/model-service');
    const panel = await screen.findByRole('tabpanel', { name: i18n.t('help.parts.screen') });
    expect(within(panel).getByText(i18n.t('screens.providers.about'))).toBeVisible();
    expect(within(panel).getAllByRole('button', { name: 'Resource guide' })).toHaveLength(1);
    expect(within(panel).queryByRole('button', { name: 'Draft guide' })).not.toBeInTheDocument();
    await userEvent.click(within(panel).getByRole('button', { name: 'Resource guide' }));
    const body = await screen.findByRole('region', { name: 'Resource guide' });
    expect(within(body).getByRole('heading', { name: 'Resource guide' })).toBeVisible();
    expect(within(body).getByText('Choose an authorized resource to see its details.')).toBeVisible();
  });

  for (const missing of ['absent', 'draft']) it(`returns from the factual missing ${missing} address to readable articles at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const router = draw(`/help/${missing}`);
    expect(await screen.findByText(i18n.t('help.articles.missing'))).toBeVisible();
    expect(screen.queryByText(/Placeholder guide|Planned coverage/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.articles.back') }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/help'));
    expect(await screen.findByRole('button', { name: 'Resource guide' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Draft guide' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Resource guide' }));
    expect(await screen.findByRole('region', { name: 'Resource guide' })).toBeVisible();
    expect(router.state.location.pathname).toBe('/help/readable');
  });
}

it('keeps the loading state until the guide arrives and never offers a draft body', () => {
  const selected = vi.fn();
  const view = show(<Articles guide={undefined} onArticle={selected} />);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  view.rerender(<Articles guide={scene.guide} onArticle={selected} />);
  expect(screen.getAllByRole('button')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Resource guide' })).toBeVisible();
  expect(selected).not.toHaveBeenCalled();
});
