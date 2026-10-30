// @vitest-environment node
/** Creation questions find the actual permission section, with stable anchors and translated interface names. */
import { describe, expect, it } from 'vitest';
import { answer, helpEntries } from '@/features/finder/local';
import { articleOf } from '@/features/help/hints';
import { guide } from '@/help/library';
import { localizedGuide } from '@/help/localized';
import { i18n, setLocale } from '@/i18n';

const article = 'who-can-see-and-do-what';
const heading = 'who-may-make-projects-topics-and-threads';

describe('finding who may create things', () => {
  it.each(['create project', 'start thread', 'make project', 'new topic', '创建项目', '开始线程'])('finds the creation section for %s', (query) => {
    const found = answer(helpEntries(guide.articles), query);
    expect(found.some(({ item }) => item.article.id === article && item.heading?.id === heading)).toBe(true);
  });
  it('keeps the section anchor while its result title follows the reader’s language', async () => {
    await setLocale('zh-CN');
    try {
      const localized = localizedGuide(guide, i18n.t);
      const found = answer(helpEntries(localized.articles), 'create project').find(({ item }) => item.heading?.id === heading);
      expect(found?.item.heading?.text).toBe('谁可以创建项目、话题和线程');
      expect(found?.item.article.id).toBe(article);
    } finally { await setLocale('en'); }
  });
  it('the three creation-permission hints lead directly to that section', () => {
    for (const kind of ['org', 'project', 'topic'] as const) expect(articleOf(`line-${kind}-interact`)).toEqual({ id: article, heading });
  });
});
