/** Language metadata stays available before the catalogue and application are loaded. */
import type { Locale } from '@/generated/api-types';

export const LOCALES: readonly { locale: Locale; name: string }[] = [
  { locale: 'en', name: 'English' },
  { locale: 'zh-CN', name: '简体中文' },
];

export function isLocale(value: unknown): value is Locale {
  return LOCALES.some((entry) => entry.locale === value);
}

/** Any Chinese is Simplified Chinese, all else English, as the interface is authored. */
export function nearestLocale(languages: readonly string[]): Locale {
  for (const language of languages) {
    if (isLocale(language)) return language;
    if (language.toLowerCase().startsWith('zh')) return 'zh-CN';
    if (language.toLowerCase().startsWith('en')) return 'en';
  }
  return 'en';
}
