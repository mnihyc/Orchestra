/** Import plan reasons are localized when known and retain the server's words otherwise. */
import { afterEach, expect, it } from 'vitest';
import { importReason } from '@/features/people/importReason';
import { i18n, setLocale } from '@/i18n';

afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('uses the authored import refusal in %s', async (locale) => {
  await setLocale(locale);
  const reason = { code: 'import.too_large', message: 'Server fallback' };
  expect(importReason(reason, i18n.t, locale)).toBe(i18n.t('errors.import.too_large'));
  expect(importReason(reason, i18n.t, locale)).not.toBe('Server fallback');
});

it('keeps the supplied sentence when a future server reason is unknown', async () => {
  await setLocale('zh-CN');
  expect(importReason({ code: 'import.future_reason', message: 'The source is unavailable' }, i18n.t, 'zh-CN')).toBe('The source is unavailable');
});

it.each(['en', 'zh-CN'] as const)('localizes every declared plan and report reason in %s', async (locale) => {
  await setLocale(locale);
  const codes = ['import.too_large', 'import.too_many', 'import.nothing', 'import.invalid', 'import.forbidden', 'import.disabled', 'import.unsupported', 'import.too_deep', 'import.unreadable', 'import.damaged', 'import.refused', 'import.parent_failed', 'native_sessions', 'workspaces', 'secrets', 'usage_and_audit', 'personal_state', 'web_images', 'not_exportable', 'too_large', 'org_as_project', 'sharing_not_restored', 'unknown_person', 'unsupported_block', 'missing_file', 'references_outside', 'warnings', 'history_dropped'];
  for (const code of codes) expect(importReason({ code, message: 'Server fallback' }, i18n.t, locale)).not.toBe('Server fallback');
});

it('formats measured limits and translates the kinds in a structured refusal', async () => {
  await setLocale('zh-CN');
  expect(importReason({ code: 'import.too_large', message: 'fallback', params: { asked: 2048, limit: 1024 } }, i18n.t, 'zh-CN')).toBe('2 KiB 超过 1 KiB 的限额');
  expect(importReason({ code: 'import.unsupported', message: 'fallback', params: { kind: 'document', parent_kind: 'thread' } }, i18n.t, 'zh-CN')).toBe('文档不能放入会话');
  expect(importReason({ code: 'missing_file', message: 'fallback', params: { name: 'map.png' } }, i18n.t, 'zh-CN')).toBe('未找到 map.png');
});


it.each([
  ['en', 'Picture not found: Maps/site & plan.png', 'Link not found, kept as words: notes/plan.md', 'There are more of these than are listed'],
  ['zh-CN', '未找到图片：Maps/site & plan.png', '未找到链接目标，按文字保留：notes/plan.md', '这类问题还有更多，未全部列出'],
] as const)('uses the shared reader vocabulary and parameters in import reports in %s', async (locale, image, link, more) => {
  await setLocale(locale);
  expect(importReason({ code: 'reader.image_missing', params: { path: 'Maps/site & plan.png' }, message: 'Server fallback' }, i18n.t, locale)).toBe(image);
  expect(importReason({ code: 'reader.link_unresolved', params: { target: 'notes/plan.md' }, message: 'Server fallback' }, i18n.t, locale)).toBe(link);
  expect(importReason({ code: 'reader.more', params: {}, message: 'Server fallback' }, i18n.t, locale)).toBe(more);
  const codes = ['tag_kept_as_text', 'link_unresolved', 'link_is_file', 'link_not_followed', 'image_missing', 'image_remote', 'image_inline', 'image_kind', 'film_missing', 'sound_missing', 'media_address', 'columns_in_column', 'column_alone', 'columns_count', 'pictures_left_out', 'unsafe_content', 'unsafe_link', 'character_reference', 'html_incomplete', 'table_span', 'media_not_address', 'word_styles', 'more'];
  for (const code of codes) {
    const sentence = importReason({ code: `reader.${code}`, params: { tag: 'aside', target: 'notes/plan.md', path: 'Maps/site & plan.png', address: 'file:///clip.mp4' }, message: 'Server fallback' }, i18n.t, locale);
    expect(sentence).not.toBe('Server fallback');
    expect(sentence).not.toContain('{{');
  }
  expect(importReason({ code: 'reader.other', params: {}, message: 'A new kind of source was kept as words.' }, i18n.t, locale)).toBe('A new kind of source was kept as words.');
});
