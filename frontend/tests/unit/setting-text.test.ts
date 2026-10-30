/** Setting names and keywords use the reader's catalogue, and numeric meanings use declared metadata rather than guessed keys. */
import { describe, expect, it } from 'vitest';
import { localizedSetting, numericSettingDisplay, numericSettingText, settingShape } from '@/features/settings/settingText';
import { i18n, loadSearchCatalogues, setLocale } from '@/i18n';
import { answer, settingEntries } from '@/features/finder/local';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import type { SettingDef as SettingWithText } from '@/generated/api-types';

const def: SettingWithText = { key: 'agent.instructions', title: 'Instructions', description: 'Fallback sentence', group: 'agent', schema: { type: 'string' }, class: 'parameter', scopes: ['project'], merge: 'append', default: '', example: 'Keep the examples current.', text_key: 'day100.builtin.agent.instructions', keywords: [] };

describe('catalogue wording', () => {
  it('uses Chinese registry wording for every built-in, without counting English fallback as a translation', async () => {
    await loadSearchCatalogues();
    await setLocale('zh-CN');
    try {
      for (const builtIn of SETTINGS_REGISTRY) {
        const localized = localizedSetting(builtIn, i18n);
        expect(localized.title, builtIn.key).toMatch(/\p{Script=Han}/u);
        expect(localized.description, builtIn.key).toMatch(/\p{Script=Han}/u);
      }
    } finally { await setLocale('en'); }
  });

  it('shows the live topic count with the correct singular and plural in both languages', async () => {
    await setLocale('en');
    expect(i18n.t('places.counts.topics', { count: 1 })).toBe('1 topic');
    expect(i18n.t('places.counts.topics', { count: 2 })).toBe('2 topics');
    await setLocale('zh-CN');
    try {
      expect(i18n.t('places.counts.topics', { count: 1 })).toBe('1 个话题');
      expect(i18n.t('places.counts.topics', { count: 2 })).toBe('2 个话题');
    } finally { await setLocale('en'); }
  });
  it('English readers can find the same setting by its Chinese keywords without changing language or its key', async () => {
    await loadSearchCatalogues();
    await setLocale('en');
    const localized = localizedSetting(def,i18n);
    const found = answer(settingEntries([localized]),'继承');
    expect(found.map(({item}) => item.key)).toEqual(['agent.instructions']);
    expect(i18n.resolvedLanguage).toBe('en');
    expect(localized.key).toBe(def.key);
  });
  it('Chinese readers search real Chinese names and keywords without changing the technical key', async () => {
    await setLocale('zh-CN');
    try {
      const localized = localizedSetting(def, i18n);
      expect(localized.key).toBe('agent.instructions');
      expect(localized.title).toBe('常驻指令');
      expect(localized.keywords.some((keywords) => keywords.includes('继承'))).toBe(true);
    } finally { await setLocale('en'); }
  });

  it('an unknown extension keeps its safe declared wording', () => {
    const extension = { ...def, key: 'extension.custom', text_key: 'missing.extension' };
    expect(localizedSetting(extension, i18n)).toBe(extension);
  });

  it('bytes and Never use metadata while zero-day removal stays immediate', () => {
    expect(settingShape({ ...def, key: 'unrelated.name', schema: { type: 'integer' }, display: { unit: 'bytes', zero: null } })).toMatchObject({ type: 'int', unit: 'bytes' });
    expect(numericSettingText(2_097_152, { unit: 'bytes', zero: null }, 'en', i18n.t)).toBe('2 MiB');
    expect(numericSettingText(0, { unit: 'days', zero: 'never' }, 'en', i18n.t)).toBe('Never');
    expect(numericSettingText(0, { unit: 'days', zero: 'off' }, 'en', i18n.t)).toBe('Off');
    expect(numericSettingText(0, { unit: 'raw', zero: 'off' }, 'en', i18n.t)).toBe('Off');
    expect(numericSettingText(0, { unit: 'days', zero: null }, 'en', i18n.t)).toBe('0 days');
  });
  it('retains a nullable share’s declared percent unit without guessing from its key', () => {
    const numeric = {...def, key: 'unrelated.name', schema: {anyOf: [{type: 'integer', 'x-unit': 'percent'}, {type: 'null'}]}};
    const display = numericSettingDisplay(numeric);
    expect(display).not.toBeNull();
    if (display === null) throw new Error('The declared unit is kept');
    expect(numericSettingText(100, display, 'en', i18n.t)).toBe('100 %');
    expect(numericSettingDisplay({...numeric, display: {unit: 'raw', zero: null}})).toEqual({unit: 'raw', zero: null});
  });

  it.each(['en', 'zh-CN'] as const)('pasted text uses declared percentages and shows Off for zero in %s', async (locale) => {
    await setLocale(locale);
    try {
      const display = { unit: 'percent', zero: 'off' } as const;
      expect(settingShape({ ...def, schema: { type: 'integer', minimum: 0, maximum: 100 }, display })).toMatchObject({ type: 'int', unit: 'percent' });
      expect(numericSettingText(25, display, locale, i18n.t)).toBe(`25 ${i18n.t('settings.editor.units.percent')}`);
      expect(numericSettingText(0, display, locale, i18n.t)).toBe(i18n.t('common.off'));
    } finally { await setLocale('en'); }
  });
});
