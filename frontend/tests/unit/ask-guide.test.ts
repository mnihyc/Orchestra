// @vitest-environment node
/** Ask's inline help and saved section links reach the same authored sections in both languages. */
import { expect, it } from 'vitest';
import { articleFirstLine, articleInLanguage } from '@/help/guide';
import { guide } from '@/help/library';
import { proseLinks, resolveArticleLink } from '@/help/links';

it('ships both Ask editions with identical canonical section destinations', () => {
  const article = guide.articles.find((article) => article.id === 'ask');
  expect(article).toBeDefined(); if (article === undefined) return;
  const expected = ['open-ask', 'write-a-question', 'read-answers-and-sources', 'longer-conversations', 'one-organization-per-conversation', 'privacy-cost-and-retention', 'start-a-permanent-question-thread', 'installation-addresses', 'ask-help-and-side-thread'];
  for (const locale of ['en', 'zh-CN']) {
    const selected = articleInLanguage(article, locale);
    expect(selected.language).toBe(locale);
    expect(selected.headings.map((heading) => heading.id)).toEqual(expected);
    expect(articleFirstLine(selected)).not.toBe('');
    for (const href of proseLinks(selected.body)) {
      const target = resolveArticleLink(href); if (target === null) continue;
      const destination = guide.articles.find((article) => article.id === target.article);
      expect(destination, href).toBeDefined();
      if (target.heading !== null && destination !== undefined) expect(articleInLanguage(destination, locale).headings.map((heading) => heading.id), href).toContain(target.heading);
    }
  }
});

it('keeps the references guide address section linked to Ask in each language', () => {
  const references = guide.articles.find((article) => article.id === 'references');
  expect(references).toBeDefined(); if (references === undefined) return;
  for (const locale of ['en', 'zh-CN']) {
    const selected = articleInLanguage(references, locale);
    expect(selected.headings.map((heading) => heading.id)).toContain('installation-addresses');
    expect(proseLinks(selected.body).map((href) => resolveArticleLink(href))).toContainEqual({ article: 'ask', heading: 'installation-addresses' });
  }
});
