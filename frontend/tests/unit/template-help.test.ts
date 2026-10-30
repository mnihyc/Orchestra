// @vitest-environment node
/** The first-template guide is bundled, searchable, translated at its entrances and guarded against missing controls or sections. */
import { describe, expect, it } from 'vitest';
import { readHelpContent } from '../../build/help-content';
import { answer, helpEntries } from '@/features/finder/local';
import { checkHelpContent } from '@/help/contentCheck';
import { localizedGuide } from '@/help/localized';
import { TEMPLATE_GUIDE } from '@/help/templates';
import { i18n, setLocale } from '@/i18n';

describe('the template guide in Help', () => {
  const content = readHelpContent();
  it('has its walkthrough sections, screen metadata and ordinary bundled article identity', () => {
    const article = content.guide.articles.find((one) => one.id === TEMPLATE_GUIDE.id);
    expect(article?.title).toBe('Write an environment template');
    expect(article?.screens).toEqual(expect.arrayContaining(['/resources/templates', '/resources/templates/$id']));
    expect(article?.headings.map((heading) => heading.id)).toEqual(expect.arrayContaining([...TEMPLATE_GUIDE.headings]));
  });
  it.each(['new template', 'environment template', 'JSON', 'env.template_id', '编写模板'])('is a finder result for %s', (query) => {
    expect(answer(helpEntries(content.guide.articles), query).some(({ item }) => item.article.id === TEMPLATE_GUIDE.id)).toBe(true);
  });
  it('uses the Chinese guide name and the same article id', async () => {
    await setLocale('zh-CN');
    try { expect(localizedGuide(content.guide, i18n.t).articles.find((one) => one.id === TEMPLATE_GUIDE.id)?.title).toBe('编写环境模板'); }
    finally { await setLocale('en'); }
  });
  it('ships the template page entrance and fails if that specific link is removed', () => {
    const path = 'features/resources/templates/Template.tsx';
    const failure = `guide.unlinked: ${TEMPLATE_GUIDE.id}:${path}`;
    expect(content.sources[path]).toContain('<TemplateGuideLink');
    expect(checkHelpContent(content)).not.toContain(failure);
    const withoutLink = (content.sources[path] ?? '').replaceAll(/<TemplateGuideLink\s*\/>/g, '');
    expect(checkHelpContent({ ...content, sources: { ...content.sources, [path]: withoutLink } })).toContain(failure);
  });
  it('fails the content gate if the article, a walkthrough section or a real entry disappears', () => {
    expect(checkHelpContent({ ...content, guide: { articles: content.guide.articles.filter((one) => one.id !== TEMPLATE_GUIDE.id) } })).toContain(`article.missing: ${TEMPLATE_GUIDE.id}`);
    const guide = { articles: content.guide.articles.map((one) => one.id === TEMPLATE_GUIDE.id ? { ...one, headings: [] } : one) };
    expect(checkHelpContent({ ...content, guide })).toContain(`heading.missing: ${TEMPLATE_GUIDE.id}#make-a-first-template`);
    const path = 'features/resources/templates/NewTemplate.tsx';
    expect(checkHelpContent({ ...content, sources: { ...content.sources, [path]: 'export {};' } })).toContain(`guide.unlinked: ${TEMPLATE_GUIDE.id}:${path}`);
  });
});
