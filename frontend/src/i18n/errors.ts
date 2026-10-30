/** Shared sentences for API errors and safe field explanations; diagnostics stay structured. */
import type { TFunction } from 'i18next';

import type { ErrorCode } from '@/generated/api-types';
import { currentLocale, i18n } from '@/i18n';

/** Protocol identifiers, settings, columns and constraints cannot serve as a sentence. */
export function hasErrorCode(text: string): boolean {
  return text.split(/[^a-z0-9_.]+/iu).some((token) => /[a-z]/iu.test(token)
    && (token.includes('_') || token.replace(/^\.+|\.+$/gu, '').includes('.')));
}

/** The typed key covers every API code; an older client still handles a future server safely. */
export function hasErrorSentence(key: string, locale: string): boolean {
  const value: unknown = i18n.getResource(locale,'translation',key);
  return typeof value === 'string';
}

/** The code stays structured; only a leaf sentence, never a catalogue object, is display copy. */
export function apiErrorSentence(code: ErrorCode, status: number, t: TFunction = i18n.t, locale: string = currentLocale()): string {
  // Before language loading, the HTTP contract's English fallback is the only available copy.
  if (!i18n.isInitialized) return 'The server could not complete this request. Try again.';
  const key = `errors.${code}` as const;
  if (hasErrorSentence(key,locale)) return t(key);
  if (status === 401) return t('errors.auth.required');
  if (status === 403) return t('errors.authz.denied');
  if (status === 404 || status === 410) return t('errors.not_found');
  if (status === 409) return t('errors.conflict.stale_revision');
  if (status === 413) return t('errors.request.too_large');
  if (status === 429) return t('errors.rate_limited');
  return status >= 500 ? t('errors.internal') : t('ui.error.requestRefused');
}

/** Keep a useful field reason and its separate path; never turn a path or code into prose. */
export function fieldErrorSentence(message: string, t: TFunction = i18n.t, path?: string): string {
  if (i18n.isInitialized && path === 'environment.use'
    && (message === 'the selected environment is unavailable'
      || message === 'the environment is not there or is being removed')
    && hasErrorSentence('errors.environment.selected_unavailable', currentLocale())) {
    return t('errors.environment.selected_unavailable');
  }
  return message.trim() !== '' && !hasErrorCode(message) ? message : t('errors.request.invalid');
}
