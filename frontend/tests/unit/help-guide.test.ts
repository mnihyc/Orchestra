// @vitest-environment node
/** The user guide as data, and what keeps the help honest: every hint has its texts in both languages, every article a hint or a link leads to is there at the heading it names, and every article belongs to a route that exists. */
import { describe, expect, it } from 'vitest';

import { helpTarget, rewriteLinks } from '@/features/help/Article';
import { HINT_IDS, articleOf, hintOf } from '@/features/help/hints';
import { belongsTo, makeGuide, readArticle, slugOf, splitFrontMatter } from '@/help/guide';
import { guide } from '@/help/library';
import { proseLinks, resolveArticleLink } from '@/help/links';
import { i18n, loadSearchCatalogues } from '@/i18n';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

describe('reading an article', () => {
  const source = ['---', 'title: Finding things', 'screens: /search, /help/$article', 'keywords: [finder, search]', '---', '# Heading one', '', 'Words.', '', '## The finder', '', '```md', '## not a heading', '```', '', '### Signs: `>` and `@`', '#### Deep', ''].join('\n');

  it('takes the title, the headings with their anchors and the front matter', () => {
    const article = readArticle('finding-things', source);
    expect(article.title).toBe('Finding things');
    expect(article.keywords).toEqual(['finder', 'search']);
    expect(article.screens).toEqual(['/search', '/help/$article']);
    expect(article.headings).toEqual([
      { id: 'the-finder', text: 'The finder', level: 2 },
      { id: 'signs-and', text: 'Signs: `>` and `@`', level: 3 },
      { id: 'deep', text: 'Deep', level: 4 },
    ]);
  });

  it('takes the title from the first heading when the front matter gives none, and the name of the file when there is no heading', () => {
    expect(readArticle('a', '# From the heading\n\ntext').title).toBe('From the heading');
    expect(readArticle('no-heading', 'just text').title).toBe('no-heading');
  });

  it('keeps authored search aliases on the section they answer', () => {
    const article = readArticle('access', '---\nheading_keywords.making-things: create project, start thread\n---\n# Access\n\n## Making things\n\n## Sharing');
    expect(article.headings).toEqual([{ id: 'making-things', text: 'Making things', level: 2, keywords: ['create project', 'start thread'] }, { id: 'sharing', text: 'Sharing', level: 2 }]);
  });

  it('keeps one canonical heading address across translated words',()=>{
    const article=readArticle('references','---\ntitle: 引用和访问权限\nheading_id.锁定的引用: locked-references\n---\n# 引用和访问权限\n\n## 锁定的引用');
    expect(article.headings).toEqual([{id:'locked-references',text:'锁定的引用',level:2}]);
  });

  it('keeps a translated section reachable by its stable heading address', () => {
    const article = readArticle('semantic-search', '---\nheading_id.索引状态: index-states\n---\n# 语义搜索\n\n## 索引状态\n');
    expect(article.headings).toEqual([{id:'index-states',text:'索引状态',level:2}]);
  });

  it('leaves an article with no front matter whole', () => {
    expect(splitFrontMatter('# T\n\ntext').body).toBe('# T\n\ntext');
    expect(splitFrontMatter('---\nunclosed').meta).toEqual({});
  });

  it('makes anchors as the hosts of Markdown do, for any script', () => {
    expect(slugOf('Find in a conversation')).toBe('find-in-a-conversation');
    expect(slugOf('What’s new?')).toBe('whats-new');
    expect(slugOf('设置与用量')).toBe('设置与用量');
  });

  it('starts with first use, then setup, and leaves extension articles stable and the README out', () => {
    const sources = { '../x/docs/user/a-extension.md': '# A', '../x/docs/user/resources.md': '# R', '../x/docs/user/README.md': '# Readme', '../x/docs/user/finding-things.md': '# Find', '../x/docs/user/getting-started.md': '# G', '../x/docs/user/workflows.md': '# Work', '../x/docs/user/z-extension.md': '# Z' };
    const ordered = ['getting-started', 'workflows', 'finding-things', 'resources', 'a-extension', 'z-extension'];
    expect(makeGuide(sources).articles.map((article) => article.id)).toEqual(ordered);
    expect(makeGuide(Object.fromEntries(Object.entries(sources).reverse())).articles.map((article) => article.id)).toEqual(ordered);
  });

  it('starts Build your own with its overview before interface contracts and worked examples', () => {
    const made = makeGuide({ 'provider-facts.md': '# Facts', 'provider-credential.md': '# Credential', 'provider-readme.md': '# Overview', 'provider-codex-plan-example.md': '# Worked example' });
    expect(made.articles.map((article) => article.id)).toEqual(['provider-readme', 'provider-credential', 'provider-facts', 'provider-codex-plan-example']);
  });

  it('belongs to a screen by the route it names or one above it', () => {
    const article = { screens: ['/settings', '/thread/$threadId'] };
    expect(belongsTo(article, '/settings/profile')).toBe(true);
    expect(belongsTo(article, '/thread/$threadId')).toBe(true);
    expect(belongsTo(article, '/inbox')).toBe(false);
  });
});

describe('the links of an article', () => {
  it('stay in the help when they lead to another article, and lead to a screen when they name one', () => {
    expect(rewriteLinks('See [the finder](finding-things.md#the-finder) and [setup](getting-started.md).')).toBe('See [the finder](/help/finding-things#the-finder) and [setup](/help/getting-started).');
    expect(rewriteLinks('[site](https://example.org/a.md) [screen](/inbox) [here](#top)')).toBe('[site](https://example.org/a.md) [screen](/inbox) [here](#top)');
    expect(helpTarget('/help/finding-things#the-finder')).toEqual({ article: 'finding-things', heading: 'the-finder' });
    expect(helpTarget('/help/getting-started')).toEqual({ article: 'getting-started', heading: null });
    expect(helpTarget('/inbox')).toBeNull();
  });
});

describe('the guide that ships with the build', () => {
  it('holds the articles of docs/user, each with a title', () => {
    expect(guide.articles.length).toBeGreaterThan(0);
    for (const article of guide.articles) expect(article.title, article.id).not.toBe('');
    expect(guide.articles.map((article) => article.id)).toContain('finding-things');
  });

  it('is led to by every hint at an article and a heading that are there', () => {
    for (const id of HINT_IDS) {
      const more = articleOf(id);
      if (more === null) continue;
      const article = guide.articles.find((candidate) => candidate.id === more.id);
      expect(article, `${id} → ${more.id}`).toBeDefined();
      if (more.heading !== null) expect(article?.headings.map((heading) => heading.id), `${id} → ${more.id}#${more.heading}`).toContain(more.heading);
    }
  });

  it('links every loss hint to its authored heading in both languages', async () => {
    await loadSearchCatalogues();
    const article = guide.articles.find((one) => one.id === 'recover-lost-work');
    expect(article).toBeDefined();
    for (const locale of ['en', 'zh-CN']) {
      const headings = locale === 'en' ? article?.headings : article?.translations?.[locale]?.headings;
      expect(headings).toBeDefined();
      for (const id of HINT_IDS.filter((one) => one.startsWith('loss-'))) {
        const destination = hintOf(id, i18n.getFixedT(locale)).article;
        expect(destination?.id).toBe('recover-lost-work');
        expect(headings?.map((one) => one.id), `${locale}:${id}`).toContain(destination?.heading);
      }
    }
  });

  it('links to no article and no heading that is not there', () => {
    for (const article of guide.articles) {
      for (const href of proseLinks(article.body)) {
        if (article.files?.[href] !== undefined) continue;
        const link = resolveArticleLink(href, article.linkPrefix);
        if (link === null) continue;
        const target = guide.articles.find((candidate) => candidate.id === link.article);
        expect(target, `${article.id} → ${link.article}`).toBeDefined();
        if (link.heading !== null) expect(target?.headings.map((heading) => heading.id), `${article.id} → ${link.article}#${link.heading}`).toContain(link.heading);
      }
    }
  });
});

describe('the hints', () => {
  const textOf = (catalogue: unknown, id: string, part: string): unknown => {
    const hints = (catalogue as { help: { hints: Record<string, Record<string, unknown> | undefined> } }).help.hints;
    return hints[id]?.[part];
  };

  it('have their title and what they say in both languages', () => {
    for (const id of HINT_IDS) {
      for (const catalogue of [english, chinese]) {
        expect(textOf(catalogue, id, 'title'), `${id}.title`).toBeTypeOf('string');
        expect(textOf(catalogue, id, 'what'), `${id}.what`).toBeTypeOf('string');
      }
      // A choice is in both languages or in neither.
      expect(textOf(english, id, 'which') === undefined, `${id}.which`).toBe(textOf(chinese, id, 'which') === undefined);
    }
  });

  it('are all there is: no text of a hint without an id in the list', () => {
    const held = Object.keys((english as { help: { hints: object } }).help.hints).sort();
    expect(held).toEqual([...HINT_IDS].sort());
  });

  it('say a thing short enough to be a hint: what it is takes two sentences at most', () => {
    for (const id of HINT_IDS) {
      const what = String(textOf(english, id, 'what'));
      expect(what.split(/[.!?]\s/).length, id).toBeLessThanOrEqual(2);
    }
  });
});
