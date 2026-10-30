/** Help offers readable articles, explains the real screen and recovers from an absent address. */
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider, useParams } from '@tanstack/react-router';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { client } from '@/api/client';
import { Articles, HelpContent } from '@/features/help/HelpView';
import { useHelpState } from '@/features/help/state';
import { makeGuide, type Guide } from '@/help/guide';
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
