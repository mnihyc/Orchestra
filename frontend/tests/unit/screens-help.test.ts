// @vitest-environment node
/** Authored lessons have a usable destination and illustration; built-in setting wording cannot silently fall back to English. */
import { describe, expect, it } from 'vitest';
import { createInstance } from 'i18next';
import { readHelpContent } from '../../build/help-content';
import { checkHelpContent, contentWord } from '@/help/contentCheck';
import { TUTORIALS, type TutorialDefinition } from '@/help/tutorials';
import { localizedSetting } from '@/features/settings/settingText';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';
import type { SettingDef as SettingWithText } from '@/generated/api-types';

const walks = ['approvals-walk', 'ports-walk', 'live-environment-walk', 'access-walk'];

describe('QS-004/005 authored steps', () => {
  it('offers a real route and an illustrated continuation at every step, in both languages', () => {
    for (const tutorial of TUTORIALS.filter((one) => walks.includes(one.id))) {
      for (const step of tutorial.steps) {
        expect(step.screen ?? step.destination, `${tutorial.id}:${step.target}`).toBeDefined();
        expect(step.example, `${tutorial.id}:${step.target}`).toBeDefined();
        for (const catalogue of [english, chinese]) {
          const text = contentWord(catalogue, step.text);
          expect(text).not.toBeNull();
          expect(text).not.toMatch(/Developers@|Team\/Web|allowed_capabilities|base_capabilities|\{"/);
          expect(contentWord(catalogue, `helpScreenReview.examples.${step.example}.title`)).not.toBeNull();
          expect(contentWord(catalogue, `helpScreenReview.examples.${step.example}.detail`)).not.toBeNull();
        }
      }
    }
  });

  it('refuses a new unreachable or unillustrated step instead of publishing a dead tutorial', () => {
    const content = readHelpContent();
    const tutorials: TutorialDefinition[] = content.tutorials.map((one) => one.id === 'approvals-walk'
      ? { ...one, steps: [{ target: 'parameters', text: 'helpRelease.walks.approvals.one' }] }
      : one);
    const failures = checkHelpContent({ ...content, tutorials });
    expect(failures).toContain('tutorial.no_destination: approvals-walk:parameters');
    expect(failures).toContain('tutorial.no_example: approvals-walk:parameters');
  });
});

describe('QS-007 exact-language lookup', () => {
  it('does not call an English fallback a Chinese translation', async () => {
    const instance = createInstance();
    await instance.init({ lng: 'zh-CN', fallbackLng: 'en', resources: {
      en: { translation: { settingRegistry: { extension: { custom: { title: 'English fallback', description: 'An English sentence.' } } } } },
      'zh-CN': { translation: {} },
    } });
    const def: SettingWithText = { key: 'extension.custom', title: 'Declared extension', description: 'The extension owns its words.', group: 'extension', schema: { type: 'boolean' }, class: 'parameter', scopes: ['thread'], merge: 'replace', default: false, example: true, text_key: null, keywords: [] };
    expect(instance.exists('settingRegistry.extension.custom.title', { lng: 'zh-CN' })).toBe(true);
    expect(localizedSetting(def, instance, 'zh-CN')).toBe(def);
  });
});
