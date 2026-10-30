/** English and Simplified Chinese catalogue loading.
 *
 * Every sentence a person reads comes from one of the two catalogues beside this file. A catalogue is
 * a part of the build of its own. English also stays available as a safe fallback.
 *
 * A key that is not in the English catalogue does not compile (`t` takes the catalogue's own keys),
 * and a unit test holds the two catalogues to the same keys and the same placeholders. While the
 * application is being developed, missing runtime text is reported without interrupting the page.
 */
/// <reference types="vite/client" />
import i18next, { type ParseKeys } from 'i18next';
import { initReactI18next } from 'react-i18next';

import type { Locale } from '@/generated/api-types';

import type english from './en.json';
import type development from './dev-en.json';

/** The catalogue every other one is held to. */
export type Catalogue = typeof english & typeof development;

declare module 'i18next' {
  interface CustomTypeOptions {
    defaultNS: 'translation';
    resources: { translation: Catalogue };
  }
}

/** A key of the catalogue, as `t` takes it. */
export type TranslationKey = ParseKeys;

/** The languages the interface is written in, each under its own name. */
export const LOCALES: readonly { locale: Locale; name: string }[] = [
  { locale: 'en', name: 'English' },
  { locale: 'zh-CN', name: '简体中文' },
];

const catalogues: Record<Locale, () => Promise<{ default: unknown }>> = {
  en: () => import('./en.json'),
  'zh-CN': () => import('./zh-CN.json'),
};

/** A gallery bundle alone does not mean the application words of that language have arrived. */
const loadedCatalogues = new Set<Locale>();
const missingWords = new Set<string>();

export function isLocale(value: unknown): value is Locale {
  return LOCALES.some((entry) => entry.locale === value);
}

/** The catalogue language nearest to what a browser asks for: any Chinese is Simplified Chinese, all else English. */
export function nearestLocale(languages: readonly string[]): Locale {
  for (const language of languages) {
    if (isLocale(language)) return language;
    if (language.toLowerCase().startsWith('zh')) return 'zh-CN';
    if (language.toLowerCase().startsWith('en')) return 'en';
  }
  return 'en';
}

async function load(locale: Locale): Promise<void> {
  if (loadedCatalogues.has(locale) && i18next.hasResourceBundle(locale, 'translation')) return;
  const catalogue = await catalogues[locale]();
  i18next.addResourceBundle(locale, 'translation', catalogue.default, true, true);
  loadedCatalogues.add(locale);
}

/** The local setting finder indexes both shipped vocabularies when it first needs the registry. */
export async function loadSearchCatalogues(): Promise<void> {
  await Promise.all(LOCALES.map(({ locale }) => load(locale)));
}

/** Starts the catalogues in `locale`. Called once, before anything is drawn. */
export async function startI18n(locale: Locale): Promise<void> {
  const warnMissing = import.meta.env.DEV;
  await i18next.use(initReactI18next).init({
    lng: locale,
    fallbackLng: 'en',
    resources: {},
    // React escapes what it draws; escaping here as well would show `&amp;` to people.
    interpolation: { escapeValue: false },
    returnNull: false,
    saveMissing: warnMissing,
    missingKeyHandler: (_languages, _namespace, key) => {
      if (warnMissing && !missingWords.has(key)) {
        missingWords.add(key);
        console.warn(`No text for "${key}" in the catalogue`);
      }
    },
  });
  await Promise.all(locale === 'en' ? [load('en')] : [load('en'), load(locale)]);
  // The bundle arrived after the start: tell what is already drawn.
  await i18next.changeLanguage(locale);
}

/** Changes the language of everything on screen, fetching its catalogue first. */
export async function setLocale(locale: Locale): Promise<void> {
  await Promise.all(locale === 'en' ? [load('en')] : [load('en'), load(locale)]);
  await i18next.changeLanguage(locale);
}

/** The language in use. */
export function currentLocale(): Locale {
  return isLocale(i18next.language) ? i18next.language : 'en';
}

export { i18next as i18n };
