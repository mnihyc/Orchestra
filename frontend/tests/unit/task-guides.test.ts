// @vitest-environment node
/** Task guides ship one identity in both languages, stay searchable and fail on broken handoffs. */
import { describe, expect, it } from 'vitest';
import { readHelpContent } from '../../build/help-content';
import { answer, helpEntries } from '@/features/finder/local';
import { checkHelpContent, contentWord } from '@/help/contentCheck';
import { articleInLanguage, articleMatchesLanguage, readArticle, type Article } from '@/help/guide';
import { TASK_GUIDES } from '@/help/tasks';

const content = readHelpContent();
function requiredArticle(id: string): Article {
  const article = content.guide.articles.find((one) => one.id === id);
  if (article === undefined) throw new Error(`missing shipped task article: ${id}`);
  return article;
}

describe('task guidance', () => {
  it.each(TASK_GUIDES)('ships $id once with authored English and Chinese bodies and names', (task) => {
    expect(content.guide.articles.filter((one) => one.id === task.id)).toHaveLength(1);
    const source = requiredArticle(task.id);
    for (const [index, language] of ['en', 'zh-CN'].entries()) {
      const article = articleInLanguage(source, language);
      expect(articleMatchesLanguage(article, language)).toBe(true);
      expect(article.title).toBe(contentWord(content.catalogues[index], task.title));
      expect(article.body).toContain(`# ${article.title}`);
      expect(article.headings.length).toBeGreaterThan(0);
      expect(article.screens.length).toBeGreaterThan(0);
      if (language === 'zh-CN') {
        expect(article.body).toMatch(/\p{Script=Han}/u);
        expect(article.body).not.toBe(source.body);
      }
    }
    expect(articleInLanguage(source, 'en').body).toBe(source.body);
  });

  it('finds the new tasks by the words people used in the reviews in either language', () => {
    for (const [id, queries] of [
      ['connect-a-machine', ['another instance', '连接机器']],
      ['add-a-provider', ['GPU', 'API key']],
      ['register-your-program', ['custom agent', '代理适配器']],
      ['temporary-environments', ['ephemeral', '临时']],
      ['message-queue', ['steer', '消息队列']],
    ] as const) for (const language of ['en', 'zh-CN']) {
      const entries = helpEntries(content.guide.articles.map((article) => articleInLanguage(article, language)));
      for (const query of queries) expect(answer(entries, query).some(({ item }) => item.article.id === id), `${language}:${query}`).toBe(true);
    }
  });

  it('selects regional Chinese and restores English without changing the source or hiding a real fallback', () => {
    const source = requiredArticle('message-queue');
    const chinese = articleInLanguage(source, 'zh-Hans');
    expect(chinese.language).toBe('zh-CN');
    expect(articleMatchesLanguage(chinese, 'zh-CN')).toBe(true);
    expect(articleInLanguage(source, 'en-US').body).toBe(source.body);
    const older = readArticle('older', '# Older article\n\nEnglish body.');
    expect(articleInLanguage(older, 'zh-CN').body).toBe(older.body);
    expect(articleMatchesLanguage(articleInLanguage(older, 'zh-CN'), 'zh-CN')).toBe(false);
  });

  it('refuses a missing translation, a dead translated link, or a guide detached from its controls', () => {
    const failures=checkHelpContent(content);
    expect(failures,failures.join('\n')).toEqual([]);
    const source = requiredArticle('message-queue');
    const translated = source.translations?.['zh-CN'];
    if (translated === undefined) throw new Error('the Chinese queue guide is required');
    const missing = { ...content, guide: { articles: content.guide.articles.map((one) => one.id === source.id ? { ...one, translations: {} } : one) } };
    expect(checkHelpContent(missing)).toContain('article.translation_missing: message-queue:zh-CN');
    const broken = { ...content, guide: { articles: content.guide.articles.map((one) => one.id === source.id
      ? { ...one, translations: { ...one.translations, 'zh-CN': { ...translated, body: `${translated.body}\n[断开的链接](missing-task.md)` } } } : one) } };
    expect(checkHelpContent(broken)).toContain('article.missing: missing-task');
    const sources = Object.fromEntries(Object.entries(content.sources).map(([path, source]) => [path,
      path.startsWith('features/') ? source.replaceAll('<TaskGuideLink guide="message-queue"', '<TaskGuideLink guide="connect-a-machine"') : source]));
    expect(checkHelpContent({ ...content, sources })).toContain('guide.unlinked: message-queue');
  });
});
