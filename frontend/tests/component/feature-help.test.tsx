/** Feature questions open real bilingual sections without changing the current control. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider, useLocation } from '@tanstack/react-router';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { PlanSwitch } from '@/features/composer/PlanSwitch';
import { ArticleView } from '@/features/help/Article';
import { Hint } from '@/features/help/Hint';
import { SettingHelpLink } from '@/features/help/SettingHint';
import { articleOf, type HintId } from '@/features/help/hints';
import { useHelpState } from '@/features/help/state';
import { ViewShapeContext } from '@/features/workbench/Narrow';
import { articleInLanguage } from '@/help/guide';
import { guide } from '@/help/library';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const initialHelp = useHelpState.getState();
afterEach(async () => {
  cleanup();
  useHelpState.setState(initialHelp);
  document.documentElement.removeAttribute('data-theme');
  await setLocale('en');
});

const hints: HintId[] = ['resource-facts', 'references', 'live-work', 'status-items', 'mount-fates', 'skills', 'plan-mode', 'plan-answer', 'switch-history', 'held-model', 'announcements', 'inbox'];

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const id of hints) {
  it(`opens ${id} in ${locale}/${theme} and keeps the Plan choice`, async () => {
    await setLocale(locale);
    document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const onPlan = vi.fn();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(['help-guide'], guide);
    const root = createRootRoute({ component: () => <QueryClientProvider client={cache}><ViewShapeContext value="wide">
      <PlanSwitch on={false} onChange={onPlan} />
      {id !== 'plan-mode' && <Hint id={id} popover />}
      {id === 'skills' && <SettingHelpLink settingKey="agents.skills" />}
    </ViewShapeContext></QueryClientProvider> });
    const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ['/'] }) });
    const view = show(<RouterProvider router={router} />);
      const name = i18n.t(`help.hint.label`, { term: i18n.t(`help.hints.${id}.title`) });
      const trigger = await screen.findByRole('button', { name });
      if (id === 'skills') {
        const settingQuestion = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('settingRegistry.agents.skills.title') }) });
        expect(settingQuestion).not.toBe(trigger);
        expect(settingQuestion).toHaveAttribute('data-help-article', 'setting:agents.skills');
      }
      await user.click(trigger);
      expect(screen.getByText(i18n.t(`help.hints.${id}.what`))).toBeVisible();
      await user.keyboard('{Escape}');
      expect(screen.queryByText(i18n.t(`help.hints.${id}.what`))).not.toBeInTheDocument();
      await user.click(trigger);
      await user.click(screen.getByRole('button', { name: i18n.t('help.hint.more') }));
      const destination = articleOf(id);
      expect(destination).not.toBeNull();
      expect(useHelpState.getState()).toMatchObject({ article: destination?.id, heading: destination?.heading });
      const source = guide.articles.find((one) => one.id === destination?.id);
      if (source === undefined || destination === null) throw new Error(`missing guide for ${id}`);
      const article = articleInLanguage(source, locale);
      const section = article.headings.find((one) => one.id === destination.heading);
      expect(section).toBeDefined();
      if (section === undefined) throw new Error(`missing section for ${id}`);
      const reader = show(<ArticleView article={article} heading={destination.heading} onArticle={() => undefined} onScreen={() => undefined} />);
      expect(screen.getByRole('heading', { name: section.text })).toHaveAttribute('id', destination.heading);
      reader.unmount();
    expect(onPlan).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: i18n.t('composer.plan.label') })).toHaveAttribute('aria-pressed', 'false');
    view.unmount();
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps a narrow question beside its control and opens its guide in ${locale}/${theme}`, async () => {
    await setLocale(locale);
    document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const onPlan = vi.fn();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(['help-guide'], guide);
    const source = guide.articles.find((one) => one.id === 'plan-before-changes');
    if (source === undefined) throw new Error('the plan guide is required');
    const article = articleInLanguage(source, locale);
    const section = article.headings.find((one) => one.id === 'plan-a-turn');
    if (section === undefined) throw new Error('the planning section is required');
    const root = createRootRoute({ component: () => <QueryClientProvider client={cache}><ViewShapeContext value="small"><Outlet /></ViewShapeContext></QueryClientProvider> });
    const index = createRoute({ getParentRoute: () => root, path: '/', component: () => <PlanSwitch on={false} onChange={onPlan} /> });
    function Reader() {
      const hash = useLocation({ select: (location) => location.hash });
      return <ArticleView article={article} heading={hash} onArticle={() => undefined} onScreen={() => undefined} />;
    }
    const reader = createRoute({ getParentRoute: () => root, path: '/help/$article', component: Reader });
    const router = createRouter({ routeTree: root.addChildren([index, reader]), history: createMemoryHistory({ initialEntries: ['/'] }) });
    show(<RouterProvider router={router} />);
    const plan = await screen.findByRole('button', { name: i18n.t('composer.plan.label') });
    const question = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('help.hints.plan-mode.title') }) });
    await user.click(question);
    expect(screen.getByRole('dialog', { name: i18n.t('help.hints.plan-mode.title') })).toBeVisible();
    expect(screen.getByRole('button', { name: i18n.t('composer.plan.label') })).toBe(plan);
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() => expect(question).toHaveFocus());
    await user.click(question);
    await user.click(screen.getByRole('button', { name: i18n.t('help.hint.more') }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/help/plan-before-changes'));
    expect(router.state.location.hash).toBe('plan-a-turn');
    expect(await screen.findByRole('heading', { name: section.text })).toHaveAttribute('id', 'plan-a-turn');
    expect(onPlan).not.toHaveBeenCalled();
  });
}
