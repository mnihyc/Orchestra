/** Build-local setting entries: the same keys, schemas and examples the server's registry serves. */
import type { i18n } from 'i18next';

import type { SettingDef } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { localizedSetting } from '@/features/settings/settingText';
import type { TranslationKey } from '@/i18n';

export const SETTING_ARTICLE_PREFIX = 'setting:';

/** A setting's help address fits the existing article route and survives a reload. */
export function settingArticle(key: string): string { return `${SETTING_ARTICLE_PREFIX}${key}`; }

export function settingKeyOf(article: string | null): string | null {
  return article?.startsWith(SETTING_ARTICLE_PREFIX) === true ? article.slice(SETTING_ARTICLE_PREFIX.length) : null;
}

/** Search settings keep their full entries and link to the authored workflow at the relevant heading. */
export function authoredSettingGuide(key: string): { article: string; heading: string; title: TranslationKey } | null {
  if (key.startsWith('related.')) return { article: 'related-items', heading: 'choose-ranking', title: 'related.title' };
  if (key.startsWith('documents.links.')) return { article: 'document-links', heading: 'choose-the-numbers', title: 'links.title' };
  const semantic: Readonly<Record<string, string>> = {
    'search.semantic.enabled': 'turn-on-semantic-search',
    'search.semantic.embeddings': 'choose-embeddings',
    'search.semantic.max_disk_share': 'disk-limit',
    'search.semantic.tuning': 'indexing-and-ranking',
    'search.word_tuning': 'indexing-and-ranking',
  };
  const heading = semantic[key];
  if (heading !== undefined) return { article: 'semantic-search', heading, title: 'help.hints.semantic-search-index.title' };
  return key.startsWith('search.') ? { article: 'finding-things', heading: 'words-and-similar-meaning', title: 'craftGuides.findingThings' } : null;
}

export function settingHelpEntries(instance: i18n): SettingDef[] {
  return SETTINGS_REGISTRY.map((def) => localizedSetting(def, instance));
}

/** A descriptor's nested parameter still opens the one registry entry that contains it. */
export function settingDefinitionForKey(defs: readonly SettingDef[], key: string): SettingDef | undefined {
  return defs.find((def) => def.key === key) ?? defs.filter((def) => key.startsWith(`${def.key}.`)).sort((one, other) => other.key.length - one.key.length)[0];
}

/** Search names, explanations, keys and authored keywords; it never searches a person's values. */
export function findSettingHelp(defs: readonly SettingDef[], query: string): SettingDef[] {
  const words = query.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);
  return defs.filter((def) => {
    const text = [def.title, def.description, def.key, def.key.replace(/[._]/g, ' '), ...def.keywords].join(' ').toLocaleLowerCase();
    return words.every((word) => text.includes(word));
  });
}
