// @vitest-environment node
/** A person finds finished guidance for setup, shared work, resource boundaries and a locked setting in Help. */
import { describe, expect, it } from 'vitest';

import { articleOf } from '@/features/help/hints';
import { articleAvailable, belongsTo } from '@/help/guide';
import { guide } from '@/help/library';

describe('guidance available beside the work', () => {
  it('offers the setup, collaboration, resource and usage articles instead of draft cards', () => {
    for (const id of ['getting-started', 'workflows', 'resources', 'settings-and-usage']) {
      const article = guide.articles.find((article) => article.id === id);
      expect(article, id).toBeDefined();
      if (article === undefined) continue;
      expect(articleAvailable(article), id).toBe(true);
      expect(article.headings.length, id).toBeGreaterThan(0);
    }
  });

  it('explains the whole environment beside a resource and a thread', () => {
    const article = guide.articles.find((article) => article.id === 'resources');
    expect(article).toBeDefined();
    if (article === undefined) return;
    expect(belongsTo(article, '/resources/environments/$id')).toBe(true);
    expect(belongsTo(article, '/thread/$threadId')).toBe(true);
    expect(article.body).toContain('whole environment');
    expect(article.body).toContain('browser profiles');
    expect(article.body.replace(/\s+/g, ' ')).toContain('sharing one document or thread does not share its folder');
  });

  it('takes a locked setting directly to its explanation', () => {
    const target = articleOf('setting-lock');
    expect(target).toEqual({ id: 'settings-and-usage', heading: 'locks' });
    const article = guide.articles.find((article) => article.id === target?.id);
    expect(article?.headings.some((heading) => heading.id === target?.heading)).toBe(true);
  });
});
