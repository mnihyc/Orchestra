/** Choice names are short terms; existing explanations and interpolated context stay separately readable. */
import { describe, expect, it } from 'vitest';
import { choiceCopy, choiceOption, offeredChoice, settingChoice, SETTING_CHOICE_GROUPS } from '@/i18n/choices';
import { i18n, setLocale } from '@/i18n';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';

function enumValues(schema: unknown): string[] {
  if (Array.isArray(schema)) return schema.flatMap((entry: unknown) => enumValues(entry));
  if (typeof schema !== 'object' || schema === null) return [];
  const node = schema as Record<string, unknown>;
  const values = Array.isArray(node.enum) ? node.enum.filter((entry: unknown): entry is string => typeof entry === 'string') : [];
  return [...values, ...Object.entries(node).filter(([key]) => key !== 'enum').flatMap(([, value]) => enumValues(value))];
}

describe('named product choices', () => {
  it('keeps retired description automation out of both the shipped registry and choice tables', async () => {
    try {
      for (const locale of ['en', 'zh-CN'] as const) {
        await setLocale(locale);
        for (const key of ['auto.description', 'auto.topic_rollup', 'docs.description_chars']) {
          expect(SETTINGS_REGISTRY.some((def) => def.key === key), `${locale}:${key}:registry`).toBe(false);
          expect(SETTING_CHOICE_GROUPS[key], `${locale}:${key}:choices`).toBeUndefined();
          expect(settingChoice(i18n.t, key, 'off'), `${locale}:${key}:off`).toBeNull();
          expect(settingChoice(i18n.t, key, 'summarizer'), `${locale}:${key}:summarizer`).toBeNull();
          expect(i18n.getResource(locale, 'translation', `settingRegistry.${key}`), `${locale}:${key}:catalogue`).toBeUndefined();
          expect(i18n.getResource(locale, 'translation', `day100.builtin.${key}`), `${locale}:${key}:builtin`).toBeUndefined();
        }
        expect(i18n.getResource(locale, 'translation', 'choiceCatalog.automation.rollup')).toBeUndefined();
        for (const def of SETTINGS_REGISTRY.filter((entry) => ['auto.title', 'auto.tags', 'auto.triggers', 'summary.targets'].includes(entry.key))) {
          for (const value of enumValues(def.schema)) {
            const copy = settingChoice(i18n.t, def.key, value);
            expect(copy?.name, `${locale}:${def.key}:${value}:retained-name`).toBeTruthy();
            expect(copy?.description, `${locale}:${def.key}:${value}:retained-description`).toBeTruthy();
          }
        }
      }
    } finally { await setLocale('en'); }
  });
  it('covers every enum offered by the shipped settings registry, including nested and list choices, in both languages', async () => {
    try {
      for (const locale of ['en', 'zh-CN'] as const) {
        await setLocale(locale);
        for (const def of SETTINGS_REGISTRY) for (const value of enumValues(def.schema)) {
          const copy = settingChoice(i18n.t, def.key, value);
          expect(copy, `${locale}:${def.key}:${value}`).not.toBeNull();
          expect(copy?.name, `${def.key}:${value}`).toBeTruthy();
          expect(copy?.description, `${def.key}:${value}`).toBeTruthy();
        }
      }
    } finally { await setLocale('en'); }
  });

  it('keeps the familiar term before the existing sentence in both languages', async () => {
    for (const locale of ['en', 'zh-CN'] as const) {
      await setLocale(locale);
      const copy = choiceCopy(i18n.t, 'approvalMode.on_request');
      expect(copy.name).toBe(i18n.t('composer.mode.labels.on_request'));
      expect(copy.description).toBe(i18n.t('choiceCatalog.approvalMode.on_request.description'));
      expect(choiceOption(i18n.t, 'approvalMode.on_request')).toEqual({ ...copy, label: copy.name });
    }
    await setLocale('en');
  });
  it('keeps the actual membership in the compact name and its explanation separate', () => {
    const copy = choiceCopy(i18n.t, 'resourceAccess.home', { home: 'My team' });
    expect(copy.name).toBe('Members of My team');
    expect(copy.description).toBe(i18n.t('resources.access.gives.home', { home: 'My team' }));
  });
  it('does not invent translated names for new plugin values', () => {
    expect(offeredChoice(i18n.t, 'serviceTier', 'vendor_custom')).toBeNull();
    expect(offeredChoice(i18n.t, 'serviceTier', 'fast')?.name).toBe('Fast');
  });
});
