/** Catalogue names and authored localized bodies, with an honest fallback for older articles. */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { type Guide, useGuide } from '@/help';
import type { TranslationKey } from '@/i18n';
import { GUIDANCE, guideWords } from '@/help/guidance';
import { PROVIDER_EXAMPLES, PROVIDER_GUIDES } from '@/help/providers';
import { TEMPLATE_GUIDE } from '@/help/templates';
import { TASK_GUIDES } from '@/help/tasks';
import { articleInLanguage } from '@/help/guide';
import { settingHelpEntries } from '@/features/help/settings';
import type { SettingDef } from '@/generated/api-types';

const TITLES: Readonly<Partial<Record<string, TranslationKey>>> = {
  updates: 'componentUpdates.guideTitle',
  'reading-views-and-details': 'readingGuide.title',
  'getting-started': 'craftGuides.gettingStarted',
  'finding-things': 'craftGuides.findingThings',
  resources: 'craftGuides.resources',
  workflows: 'craftGuides.workflows',
  'settings-and-usage': 'craftGuides.settingsAndUsage',
  [TEMPLATE_GUIDE.id]: TEMPLATE_GUIDE.title,
  ...Object.fromEntries(TASK_GUIDES.map((guide) => [guide.id, guide.title])),
  ...Object.fromEntries(PROVIDER_GUIDES.map((guide) => [guide.id, guide.title])),
  ...Object.fromEntries(PROVIDER_EXAMPLES.map((example) => [example.id, example.title])),
  ...Object.fromEntries(GUIDANCE.map((guide) => [guide.id, guide.title])),
};

/** An article name is interface wording; its body is authored content, which may still be English. */
export function localizedGuide(guide: Guide, say: (key: TranslationKey) => string, settings: readonly Pick<SettingDef, 'key' | 'title' | 'keywords'>[] = [], language = 'en'): Guide {
  return {
    ...guide,
    articles: guide.articles.map((source) => {
      const article = articleInLanguage(source, language);
      const key = TITLES[article.id];
      const headings = article.id === 'who-can-see-and-do-what' ? article.headings.map((heading) => heading.id === 'who-may-make-projects-topics-and-threads'
        ? { ...heading, text: say('creationPermissions.section') } : heading) : article.headings;
      const named = GUIDANCE.find((one) => one.id === article.id)?.settings ?? [];
      const keywords = [...article.keywords, ...guideWords(article.id), ...(PROVIDER_EXAMPLES.find((example) => example.id === article.id)?.words ?? []), ...(article.id === TEMPLATE_GUIDE.id ? TEMPLATE_GUIDE.words : []), ...settings.filter((def) => named.includes(def.key)).flatMap((def) => [def.title, ...def.keywords])];
      return key === undefined ? { ...article, headings, keywords } : { ...article, headings, keywords, title: say(key) };
    }),
  };
}

/** Every Help or finder entrance reads the same names, and a language change updates those names. */
export function useLocalizedGuide(enabled = true): Guide | undefined {
  const { t, i18n } = useTranslation();
  const guide = useGuide(enabled);
  const language = i18n.resolvedLanguage ?? i18n.language;
  return useMemo(() => guide === undefined ? undefined : localizedGuide(guide, t, settingHelpEntries(i18n), language), [guide, t, i18n, language]);
}
