/** Workbench choices expose a familiar term and an explanation in both shipped languages. */
import { createInstance } from 'i18next';
import { MessageSquare } from 'lucide-react';
import { beforeAll, expect, it } from 'vitest';

import { containerChoice, viewChoice } from '@/features/workbench/choiceWords';
import type { ViewDefinition } from '@/features/workbench/views';
import en from '@/i18n/en.json';
import zh from '@/i18n/zh-CN.json';

const language = createInstance();
beforeAll(async () => { await language.init({ lng: 'en', fallbackLng: false, resources: { en: { translation: en }, 'zh-CN': { translation: zh } } }); });
const view: ViewDefinition = { name: 'chat', title: 'views.chat.title', about: 'views.chat.about', icon: MessageSquare, screens: 'all', area: 'right', body: () => null };

it.each([['en', 'Chat'], ['zh-CN', '聊天']] as const)('names a view before describing it in %s', (locale, name) => {
  const t = language.getFixedT(locale);
  const choice = viewChoice(view, t);
  expect(choice.name).toBe(name);
  expect(choice.description).toBe(t('views.chat.about'));
  expect(choice.description).not.toBe(choice.name);
});

it.each(['en', 'zh-CN'] as const)('resolves every built-in choice pair into words in %s', (locale) => {
  const t = language.getFixedT(locale);
  for (const name of Object.keys(en.workbench.choices.views)) {
    const choice = viewChoice({ ...view, name }, t);
    expect(choice.name).not.toBe('');
    expect(choice.description).not.toBe('');
    // A sentence can end in "files."; an untranslated key has another segment after its dot.
    expect(choice.name + choice.description).not.toMatch(/\$t\(|(?:workbench|views|threadViews|navigation|providerPlan|ports|browser|vnc|files|search)\.[\w-]+/);
  }
  for (const name of Object.keys(en.workbench.choices.containers)) {
    const choice = containerChoice(name, t);
    expect(choice.name).not.toBe('');
    expect(choice.description).not.toBe('');
    expect(choice.name + choice.description).not.toMatch(/\$t\(|(?:workbench|views|threadViews|navigation|providerPlan|ports|browser|vnc|files|search)\.[\w-]+/);
  }
});

it('keeps an extension view\'s contributed name and description', () => {
  const choice = viewChoice({ ...view, name: 'extension-chat' }, language.getFixedT('en'));
  expect(choice).toEqual({ name: 'Chat', description: en.views.chat.about });
});
