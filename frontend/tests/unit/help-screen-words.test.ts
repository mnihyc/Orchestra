// @vitest-environment node
/** Every screen offered by the frame has actual words and a readable article, including detail routes. */
import { expect, it } from 'vitest';
import { readHelpContent } from '../../build/help-content';
import { screenAbout } from '@/features/help/screenWords';
import { checkHelpContent, contentWord } from '@/help/contentCheck';
import { articleAvailable, belongsTo } from '@/help/guide';

const content = readHelpContent();
const screens = Object.entries(content.sources)
  .filter(([file, source]) => file.startsWith('routes/') && /\bstaticData:\s*\{[^}]*\bscreen:/.test(source))
  .flatMap(([, source]) => Array.from(source.matchAll(/createFileRoute\('([^']+)'\)/g), (match) => match[1] ?? ''));

it('has screen words in both catalogues and a complete related article for every framed route', () => {
  expect(screens).toEqual(expect.arrayContaining([
    '/', '/resources/providers/$id/', '/doc/$documentId/', '/admin/$section/', '/u/$username',
  ]));
  for (const route of screens) {
    const about = screenAbout(route);
    expect(about, route).toBeDefined();
    if (about === undefined) throw new Error(`No screen words for ${route}`);
    for (const catalogue of content.catalogues) expect(contentWord(catalogue, about), route).not.toBeNull();
    const normalized = route === '/' ? '/' : route.replace(/\/$/, '');
    expect(content.guide.articles.some((article) => articleAvailable(article) && belongsTo(article, normalized)), route).toBe(true);
  }
});

it('uses the most specific screen, while resource details keep their actual kind', () => {
  expect(screenAbout('/resources/providers/$id/')).toBe('screens.providers.about');
  expect(screenAbout('/org/$slug/settings/$section/')).toBe('screens.orgSettings.about');
  expect(screenAbout('/project/$projectId/settings/')).toBe('screens.projectSettings.about');
  expect(screenAbout('/help/$article/')).toBe('screens.helpArticle.about');
  expect(screenAbout('/settings/your-page/')).toBe('screens.profile.about');
  expect(screenAbout('/unknown-screen/')).toBeUndefined();
});

it('refuses a screen whose article is absent or only draft scaffolding', () => {
  const guide = { articles: content.guide.articles.map((article) => belongsTo(article, '/everything')
    ? { ...article, body: 'Placeholder guide.\n\nPlanned coverage: finding things.' } : article) };
  expect(checkHelpContent({ ...content, guide })).toContain('screen.article_missing: /everything');
  const missing = { articles: content.guide.articles.filter((article) => !belongsTo(article, '/doc/$documentId')) };
  expect(checkHelpContent({ ...content, guide: missing })).toContain('screen.article_missing: /doc/$documentId');
});

it('refuses a missing screen sentence in either catalogue', () => {
  for (const catalogues of [[{}, content.catalogues[1]], [content.catalogues[0], {}]]) {
    expect(checkHelpContent({ ...content, catalogues })).toEqual(expect.arrayContaining([
      expect.stringMatching(/^text\.missing: [01]:screens\.providers\.about$/),
    ]));
  }
});

it('refuses a new framed route without screen words instead of offering an empty screen tab', () => {
  const sources = { ...content.sources, 'routes/new/index.tsx': "createFileRoute('/new-screen/')({ staticData: { screen: 'page' } })" };
  expect(checkHelpContent({ ...content, sources, routes: [...content.routes, '/new-screen/'] })).toContain('screen.words_missing: /new-screen/');
});
