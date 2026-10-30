// @vitest-environment node
/** Reading guidance cannot ship without both authored editions and the native screen entrance. */
import { expect, it } from 'vitest';
import { readHelpContent } from '../../build/help-content';
import { checkHelpContent } from '@/help/contentCheck';
import { READING_GUIDE, readingGuideHeading } from '@/help/reading';
import { articleOf } from '@/features/help/hints';

it('validates both editions and rejects a removed Chinese heading or screen entrance', () => {
  const content = readHelpContent();
  expect(checkHelpContent(content)).toEqual([]);
  const article = content.guide.articles.find((one) => one.id === READING_GUIDE.id);
  expect(article?.translations?.['zh-CN']?.body).toContain('卡片和菜单');
  for (const section of ['cards', 'finder'] as const) {
    const heading = readingGuideHeading('zh-CN', section);
    const guide = { articles: content.guide.articles.map((one) => {
      if (one.id !== READING_GUIDE.id || one.translations?.['zh-CN'] === undefined) return one;
      const translated = one.translations['zh-CN'];
      return { ...one, translations: { ...one.translations, 'zh-CN': { ...translated, headings: translated.headings.filter((item) => item.id !== heading) } } };
    }) };
    expect(checkHelpContent({ ...content, guide })).toContain(`heading.missing: ${READING_GUIDE.id}#${heading}`);
  }
  const sources = { ...content.sources, 'features/workbench/Arrangements.tsx': 'export {};' };
  expect(checkHelpContent({ ...content, sources })).toContain(`guide.unlinked: ${READING_GUIDE.id}`);
});

for (const language of ['en', 'zh-CN']) it('the finder hint reaches its authored mode explanation in ' + language, () => {
  const content = readHelpContent();
  const article = content.guide.articles.find((one) => one.id === READING_GUIDE.id);
  const target = articleOf('finder-modes', language);
  const edition = language === 'en' ? article : article?.translations?.[language];
  expect(target).toEqual({ id: READING_GUIDE.id, heading: readingGuideHeading(language, 'finder') });
  expect(edition?.headings.find((heading) => heading.id === target?.heading)?.keywords).toContain(language === 'en' ? 'All' : '全部');
  expect(edition?.body).toContain(language === 'en' ? '**All**' : '**全部**');
});
