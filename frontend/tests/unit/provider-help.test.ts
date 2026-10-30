// @vitest-environment node
/** Implementer help is a shipped section with real namespaces, local schemas and strict content checks. */
import { describe, expect, it } from 'vitest';

import { readHelpContent } from '../../build/help-content';
import { checkHelpContent } from '@/help/contentCheck';
import { makeGuide, readArticle } from '@/help/guide';
import { proseLinks, resolveArticleLink, rewriteGuideLinks } from '@/help/links';
import { PROVIDER_EXAMPLES, providerGuidesForSetting } from '@/help/providers';

function textAt(catalogue: unknown, path: string): unknown {
  let node = catalogue;
  for (const part of path.split('.')) {
    if (typeof node !== 'object' || node === null) return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return node;
}

describe('provider implementer help', () => {
  it('indexes all worked provider programs with translated titles and bilingual search words', () => {
    const content = readHelpContent();
    for (const example of PROVIDER_EXAMPLES) {
      const article = content.guide.articles.find((one) => one.id === example.id);
      expect(article?.section, example.id).toBe('build-your-own');
      expect(article?.body, example.id).toContain('examples/plugins/provider/');
      expect(example.words.some((word) => /\p{Script=Han}/u.test(word)), example.id).toBe(true);
      for (const [locale, catalogue] of content.catalogues.entries()) {
        const title = textAt(catalogue, example.title);
        expect(typeof title, `${locale}:${example.id}`).toBe('string');
        expect(title, `${locale}:${example.id}`).toBeTruthy();
        if (locale === 1) expect(title).toMatch(/\p{Script=Han}/u);
      }
    }
  });

  it('keeps the provider README and disambiguates same-named articles', () => {
    const guide = makeGuide({
      'docs/user/facts.md': '# My facts',
      'docs/development/extensions/providers/README.md': '---\nid: provider-readme\nsection: build-your-own\nlink_prefix: provider-\n---\n# Build your own provider source',
      'docs/development/extensions/providers/facts.md': '---\nid: provider-facts\nsection: build-your-own\nlink_prefix: provider-\n---\n# Provider facts',
    }, { 'schema/source-ask.json': '/assets/source-ask.json' });
    expect(guide.articles.map((article) => article.id)).toEqual(['provider-readme', 'provider-facts', 'facts']);
    expect(guide.articles.find((article) => article.id === 'provider-facts')).toMatchObject({ section: 'build-your-own', linkPrefix: 'provider-', files: { 'schema/source-ask.json': '/assets/source-ask.json' } });
    expect(resolveArticleLink('README.md#register-a-program', 'provider-')).toEqual({ article: 'provider-readme', heading: 'register-a-program' });
    expect(resolveArticleLink('../development/extensions/providers/models.md')).toEqual({ article: 'provider-models', heading: null });
    expect(resolveArticleLink('../../../../user/provider-details.md', 'provider-')).toEqual({ article: 'provider-details', heading: null });
  });

  it('preserves exact worked code while rewriting prose and schema links', () => {
    const code = '```python\nprint("[not a link](missing.md)")\n```';
    const text = `[models](models.md) [schema](schema/source-ask.json)\n${code}`;
    const rewritten = rewriteGuideLinks(text, 'provider-', { 'schema/source-ask.json': '/assets/source-ask.json' });
    expect(rewritten).toContain('[models](/help/provider-models)');
    expect(rewritten).toContain('[schema](/assets/source-ask.json)');
    expect(rewritten).toContain(code);
    expect(proseLinks(text)).toEqual(['models.md', 'schema/source-ask.json']);
    expect(proseLinks('````md\n```\n[code](missing.md)\n````\n[models](models.md)')).toEqual(['models.md']);
  });

  it('fails when an interface, schema or product entrance is missing', () => {
    const content = readHelpContent();
    const failures = checkHelpContent(content);
    expect(failures, failures.join('\n')).toEqual([]);
    const missing = { ...content, guide: { articles: content.guide.articles.filter((article) => article.id !== 'provider-models') } };
    expect(checkHelpContent(missing)).toContain('provider.article_missing: provider-models');
    const files = { ...content, guide: { articles: content.guide.articles.map((article) => article.id === 'provider-facts' ? { ...article, files: {} } : article) } };
    expect(checkHelpContent(files)).toContain('file.missing: provider-facts:schema/source-ask.json');
    const sources = Object.fromEntries(Object.entries(content.sources).map(([path, source]) => [path, path.startsWith('features/') && !path.startsWith('features/help/') ? source.replaceAll('<ProviderGuideLink guide="provider-facts"', '<ProviderGuideLink guide="provider-models"') : source]));
    expect(checkHelpContent({ ...content, sources })).toContain('guide.unlinked: provider-facts');
  });

  it('finds real state and command vocabulary and keeps secret update semantics explicit', () => {
    const content = readHelpContent();
    const facts = content.guide.articles.find((article) => article.id === 'provider-facts');
    const commands = content.guide.articles.find((article) => article.id === 'commands-after-start');
    expect(facts?.keywords).toContain('FactsReport');
    expect(facts?.body).toContain('previous value and observation time');
    expect(commands?.body).toContain('only its owners');
    expect(providerGuidesForSetting('runtime.after_up')).toEqual(['commands-after-start']);
    expect(providerGuidesForSetting('providers.allow_http_upstreams')).toEqual(['provider-details']);
    expect(readArticle('x', '# X').section).toBeUndefined();
  });
});
