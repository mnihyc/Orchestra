// @vitest-environment node
/** Every router screen has explicit help and every user article has an authored, addressable Chinese edition. */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { readHelpContent } from '../../build/help-content';
import { checkHelpContent } from '@/help/contentCheck';

const content = readHelpContent();

describe('complete screen and language coverage', () => {
  it('covers every generated router destination, including account pages, redirects and the gallery', () => {
    const router = readFileSync(new URL('../../src/routeTree.gen.ts', import.meta.url), 'utf8');
    const destinations = /export interface FileRoutesByTo \{([\s\S]*?)\n\}/.exec(router)?.[1] ?? '';
    const routes = [...destinations.matchAll(/'([^']+)':/g)].map((one) => one[1]).sort();
    expect(routes.length).toBeGreaterThan(0);
    expect(content.routes.map((route) => route === '/' ? '/' : route.replace(/\/$/, '')).sort()).toEqual(routes);
    const failures = checkHelpContent(content);
    expect(failures, failures.join('\n')).toEqual([]);
  });

  it('names a new route without help even when its parent already has an article', () => {
    const failures = checkHelpContent({ ...content, routes: [...content.routes, '/settings/new-screen/'] });
    expect(failures).toContain('screen.article_missing: /settings/new-screen');
  });

  it('names a missing Chinese edition of any user article, including older guides', () => {
    const guide = { articles: content.guide.articles.map((one) => one.id === 'resources' ? { ...one, translations: {} } : one) };
    expect(checkHelpContent({ ...content, guide })).toContain('article.translation_missing: resources:zh-CN');
  });

  it('refuses copied English prose as a translation', () => {
    const guide = { articles: content.guide.articles.map((one) => one.id === 'references' ? { ...one, translations: { 'zh-CN': one } } : one) };
    expect(checkHelpContent({ ...content, guide })).toContain('article.translation_missing: references:zh-CN');
  });

  it('names a missing translated section so a screen hint cannot silently open the article top', () => {
    const guide = { articles: content.guide.articles.map((one) => {
      const chinese = one.translations?.['zh-CN'];
      return one.id !== 'plan-before-changes' || chinese === undefined ? one : { ...one, translations: { 'zh-CN': { ...chinese, headings: chinese.headings.filter((heading) => heading.id !== 'decide-the-plan') } } };
    }) };
    expect(checkHelpContent({ ...content, guide })).toContain('article.translation_heading_missing: plan-before-changes#decide-the-plan:zh-CN');
  });

  it('refuses an absent heading named by a screen guide', () => {
    const sources = { ...content.sources, 'features/example/Help.tsx': '<ArticleGuideLink article="references" heading="missing-section" />' };
    expect(checkHelpContent({ ...content, sources })).toContain('heading.missing: references#missing-section');
  });
});
