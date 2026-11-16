/** English and Simplified Chinese catalogue loading.
 *
 * Every sentence a person reads comes from one of the two catalogues beside this file. A catalogue is
 * a part of the build of its own. English also stays available as a safe fallback.
 *
 * A syntax-only key gate runs before the compiler; finite metadata keys stay typed without giving
 * i18next the whole catalogue at every call. Tests hold both languages to the same keys and placeholders. While the
 * application is being developed, missing runtime text is reported without interrupting the page.
 */
/// <reference types="vite/client" />
import i18next from 'i18next';
import { initReactI18next } from 'react-i18next';

import type { Locale } from '@/generated/api-types';

import type english from './en.json';
import type development from './dev-en.json';

/** The catalogue every other one is held to. */
export type Catalogue = typeof english & typeof development;

/** Metadata keeps a finite key vocabulary, evaluated once rather than by each t() overload. */
type LeafKeys<Value> = Value extends readonly unknown[] ? never : Value extends object ? {
  [Key in keyof Value & string]: Value[Key] extends string ? Key : `${Key}.${LeafKeys<Value[Key]>}`;
}[keyof Value & string] : never;
type Plural = 'zero' | 'one' | 'two' | 'few' | 'many' | 'other';
type BaseKey<Key extends string> = Key extends `${infer Base}_ordinal_${Plural}` ? Base
  : Key extends `${infer Base}_${Plural}` ? Base : never;
type ExactKey = LeafKeys<Catalogue>;
/** Authored metadata is checked here; t() calls are checked by build/check-translation-keys.ts. */
export type TranslationKey = ExactKey | BaseKey<ExactKey>;

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

/**
 * Waits for the browser to be idle, and for `ms` at the latest. What is fetched for safety, and not for the first draw, is fetched
 * in this way, so that it never stands beside what the page is showing.
 */
function whenIdle(work: () => void, ms: number): void {
  if (typeof requestIdleCallback === 'function') requestIdleCallback(work, { timeout: ms });
  else setTimeout(work, ms);
}

/** How long after the first draw English is fetched as the fallback of another language, if the browser has not been idle before. */
export const FALLBACK_WORDS_AFTER_MS = 2000;

/**
 * Starts the catalogues in `locale`. Called once, before anything is drawn. The reader's own words are all that the first draw
 * needs (the two catalogues hold the same keys, and tests keep them so); English, the fallback, is fetched once the browser is
 * idle, so that a reader of another language does not wait for two catalogues.
 */
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
  await load(locale);
  // The bundle arrived after the start: tell what is already drawn.
  await i18next.changeLanguage(locale);
  if (locale !== 'en') whenIdle(() => { void load('en').catch(() => undefined); }, FALLBACK_WORDS_AFTER_MS);
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
