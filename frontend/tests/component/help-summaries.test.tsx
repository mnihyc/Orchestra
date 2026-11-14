/** Written summaries head the real article and its result rows without replacing names or anchors. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ArticleView } from '@/features/help/Article';
import { Articles } from '@/features/help/HelpView';
import { HelpResult } from '@/features/help/HelpResult';
import { articleInLanguage, makeGuide, readArticle } from '@/help/guide';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
const ask = vi.hoisted(() => vi.fn());
vi.mock('@/features/ask/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), openAskWithDraft: ask }));

const source = makeGuide({ 'plan.md': '---\nsummary: Review the plan before moving work.\n---\n# Plan\n\nThe opening remains separate.\n\n## Next\n\nContinue here.' }, {}, {
  'zh-CN': { 'plan.md': '---\nsummary: 移动工作前先查看计划。\nheading_id.下一步: next\n---\n# 计划\n\n原有开头单独保留。\n\n## 下一步\n\n从这里继续。' },
}).articles[0];
if (source === undefined) throw new Error('The bilingual article is required');
beforeEach(() => { Element.prototype.scrollIntoView = vi.fn(); ask.mockClear(); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`opens Ask with the actual article, heading and authored language (${locale})`, async () => {
    await setLocale(locale);
    const article = articleInLanguage(source, locale);
    show(<ArticleView article={article} heading="next" onArticle={vi.fn()} onScreen={vi.fn()} />);
    expect(ask).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.ask') }));
    expect(ask).toHaveBeenCalledExactlyOnceWith({ text: `${i18n.t('summaryBox.ask')}\n\n${article.title}\n/help/${article.id}#next\n> ${article.summary ?? ''}` });
  });
  it(`places the authored paragraph below the title and preserves the section destination (${locale})`, async () => {
    await setLocale(locale);
    const article = articleInLanguage(source, locale);
    const view = show(<ArticleView article={article} heading="next" onArticle={vi.fn()} onScreen={vi.fn()} />);
    const title = screen.getByRole('heading', { level: 1 });
    expect(title.nextElementSibling).toHaveAttribute('data-summary-text');
    expect(title.nextElementSibling).toHaveTextContent(article.summary ?? '');
    expect(view.container.querySelectorAll('[data-summary-text]')).toHaveLength(1);
    expect(screen.getByText(locale === 'en' ? 'The opening remains separate.' : '原有开头单独保留。')).toBeVisible();
    expect(screen.getByRole('heading', { name: locale === 'en' ? 'Next' : '下一步' })).toHaveAttribute('id', 'next');
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.refresh') })).not.toBeInTheDocument();
  });

  it(`uses the written paragraph as the directory description while keeping its name and destination (${locale})`, async () => {
    await setLocale(locale);
    const article = articleInLanguage(source, locale);
    const open = vi.fn();
    show(<Articles guide={{ articles: [article] }} onArticle={open} />);
    const button = screen.getByRole('button', { name: article.title });
    expect(button).toHaveAccessibleDescription(article.summary);
    expect(button).not.toHaveTextContent(locale === 'en' ? 'The opening remains separate.' : '原有开头单独保留。');
    await userEvent.click(button);
    expect(open).toHaveBeenCalledExactlyOnceWith(article.id);
  });

  it(`gives a heading hit its parent name and the article summary (${locale})`, async () => {
    await setLocale(locale);
    const article = articleInLanguage(source, locale);
    const heading = article.headings[0];
    if (heading === undefined) throw new Error('The article section is required');
    const open = vi.fn();
    show(<ul><HelpResult article={article} heading={heading} onOpen={open} /></ul>);
    const button = screen.getByRole('button', { name: heading.text });
    expect(button).toHaveAccessibleDescription(`${article.title} ${article.summary ?? ''}`);
    expect(button).toHaveTextContent(article.summary ?? '');
    await userEvent.click(button);
    expect(open).toHaveBeenCalledOnce();
  });
}

it('keeps an article summary distinct from its same-named directory group', () => {
  const article = readArticle('getting-started', '---\ngroup: getting-started\nsummary: Set up a place and begin work.\n---\n# Getting started\n\nAn introduction.');
  const view = show(<Articles guide={{ articles: [article] }} onArticle={vi.fn()} />);
  const button = screen.getByRole('button', { name: article.title });
  const heading = screen.getByRole('heading', { name: i18n.t('help.articleGroups.getting-started') });
  expect(button).toHaveAccessibleDescription(article.summary);
  expect(button.getAttribute('aria-describedby')).not.toBe(heading.id);
  const ids = [...view.container.querySelectorAll('[id]')].map((element) => element.id);
  expect(new Set(ids).size).toBe(ids.length);
});

it('inserts one summary after a real title rather than the heading text in a code fence', () => {
  const article = readArticle('fences', '---\nsummary: A written summary.\n---\n```md\n# Only code\n```\n\n# Real title\n\nOpening text.\n\n# Another title');
  const view = show(<ArticleView article={article} heading={null} onArticle={vi.fn()} onScreen={vi.fn()} />);
  const title = screen.getByRole('heading', { name: 'Real title' });
  expect(title.nextElementSibling).toHaveAttribute('data-summary-text');
  expect(view.container.querySelectorAll('[data-summary-text]')).toHaveLength(1);
  expect(screen.queryByRole('heading', { name: 'Only code' })).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Another title' })).toBeVisible();
});

it('keeps articles with no authored summary and their existing preview readable', () => {
  const article = readArticle('plain', '# Plain\n\nAn existing introduction.');
  const view = show(<ArticleView article={article} heading={null} onArticle={vi.fn()} onScreen={vi.fn()} />);
  expect(view.container.querySelector('[data-summary-text]')).toBeNull();
  expect(screen.getByText('An existing introduction.')).toBeVisible();
});
