// @vitest-environment node
/** Help summaries are authored, local to their language and checked with the shipped content. */
import { readFileSync, readdirSync } from 'node:fs';
import { expect, it, vi } from 'vitest';
import { readHelpContent } from '../../build/help-content';
import { checkHelpContent } from '@/help/contentCheck';
import { articleInLanguage, articlePreview, makeGuide, readArticle, splitFrontMatter } from '@/help/guide';
import { answer, helpEntries, helpRows } from '@/features/finder/local';
import type { Env } from '@/features/finder/rows';
import { i18n, loadSearchCatalogues } from '@/i18n';

it.each(['en', 'zh-CN'] as const)('every user-guide file has its own plain written summary (%s)', (language) => {
  const directory = new URL(language === 'en' ? '../../../docs/user/' : '../../../docs/user/zh-CN/', import.meta.url);
  for (const name of readdirSync(directory).filter((one) => one.endsWith('.md'))) {
    const { meta } = splitFrontMatter(readFileSync(new URL(name, directory), 'utf8'));
    expect(meta.summary, `${language}/${name}`).toEqual(expect.any(String));
    expect(meta.summary?.trim(), `${language}/${name}`).not.toBe('');
    expect(meta.summary, `${language}/${name}`).not.toMatch(/[\r\n]|<[^>]+>|\[[^\]]*\]\(|\*\*|`/u);
    if (language === 'zh-CN') expect(meta.summary, name).toMatch(/\p{Script=Han}/u);
  }
});

it('preserves both authored summaries without reading a paragraph or falling back across languages', () => {
  const english = '---\nsummary: Review the plan before moving work.\n---\n# Move work\n\nA different opening paragraph.';
  const chinese = '---\nsummary: 移动工作前先查看计划。\n---\n# 移动工作\n\n另一段开头。';
  const guide = makeGuide({ 'move.md': english }, {}, { 'zh-CN': { 'move.md': chinese } });
  const article = guide.articles[0];
  if (article === undefined) throw new Error('The authored guide is missing');
  expect(articleInLanguage(article, 'en').summary).toBe('Review the plan before moving work.');
  expect(articleInLanguage(article, 'zh-CN').summary).toBe('移动工作前先查看计划。');
  const missing = { ...article, translations: { 'zh-CN': { title: '移动工作', body: '正文', headings: [], keywords: [] } } };
  expect(articleInLanguage(missing, 'zh-CN').summary).toBeUndefined();
  expect(readArticle('empty', '# No summary\n\nAn opening paragraph.').summary).toBeUndefined();
});

it.each(['en', 'zh-CN'] as const)('the content gate refuses a missing authored summary (%s)', (language) => {
  const content = readHelpContent();
  const id = 'getting-started';
  const guide = { articles: content.guide.articles.map((article) => {
    if (article.id !== id) return article;
    if (language === 'en') {
      const changed = { ...article };
      delete changed.summary;
      return changed;
    }
    const chinese = article.translations?.['zh-CN'];
    if (chinese === undefined) throw new Error('The authored Chinese guide is missing');
    const changed = { ...chinese };
    delete changed.summary;
    return { ...article, translations: { ...article.translations, 'zh-CN': changed } };
  }) };
  expect(checkHelpContent({ ...content, guide })).toContain(`article.summary_missing: ${id}:${language}`);
});

it('prefers written preview text while keeping the introduction fallback for other guides', () => {
  expect(articlePreview({ summary: 'Written summary.', body: '# Title\n\nThe opening paragraph.' })).toBe('Written summary.');
  expect(articlePreview({ body: '# Title\n\nThe opening paragraph.' })).toBe('The opening paragraph.');
});

it.each(['en', 'zh-CN'] as const)('finds an article through its written summary and retains the heading identity (%s)', async (locale) => {
  await loadSearchCatalogues();
  const summary = locale === 'en' ? 'A uniquely searchable summary.' : '一段独有的摘要内容。';
  const article = readArticle('summary-search', `---\nsummary: ${summary}\n---\n# Guide\n\nAn unrelated opening.\n\n## Section\n\nOther prose.`);
  const entries = helpEntries([article]);
  expect(answer(entries, summary).map(({ item }) => item.heading?.id ?? item.article.id)).toEqual(['summary-search']);
  const env: Env = { t: i18n.getFixedT(locale), go: vi.fn(), orgSlug: 'example', exists: () => true, showCard: vi.fn() };
  const open = vi.fn();
  const row = helpRows(entries, 'Section', env, open)[0]?.row;
  expect(row?.title).toBe('Section');
  expect(row?.detail).toBe(`Guide · ${summary}`);
  expect(row?.note).toBe(env.t('finder.help.articleNote'));
  row?.run('here');
  expect(open).toHaveBeenCalledExactlyOnceWith('summary-search', 'section');
});
