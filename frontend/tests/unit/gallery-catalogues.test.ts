/** Production words omit the development gallery while its own catalogues preserve both languages. */
import { describe, expect, it } from 'vitest';
import en from '@/i18n/en.json';
import zh from '@/i18n/zh-CN.json';
import devEn from '@/i18n/dev-en.json';
import devZh from '@/i18n/dev-zh-CN.json';
import { i18n, setLocale } from '@/i18n';
import { installDevCatalogue } from '@/i18n/dev';

describe('the gallery’s separate words', () => {
  it('keeps the gallery out of both shipped application catalogues', () => {
    expect(en).not.toHaveProperty('dev');
    expect(zh).not.toHaveProperty('dev');
  });
  it('makes the gallery’s examples readable in both languages only when its words are installed', async () => {
    installDevCatalogue();
    await setLocale('en');
    expect(i18n.t('dev.title')).toBe(devEn.dev.title);
    await setLocale('zh-CN');
    expect(i18n.t('dev.title')).toBe(devZh.dev.title);
    expect(i18n.t('common.cancel')).toBe(zh.common.cancel);
    await setLocale('en');
  });
});
