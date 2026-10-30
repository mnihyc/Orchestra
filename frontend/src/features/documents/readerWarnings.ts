/** What a reader says of what it kept as words or left out, in the reader's own language.
 *
 * The server gives each as a stable code with its parameters and an English sentence. A code the catalogue knows is said
 * in its words, in either language; one it does not know is said in the server's sentence, as errors are.
 */
import type { TFunction } from 'i18next';

import type { ReaderWarning } from '@/generated/api-types';
import { currentLocale, i18n, type TranslationKey } from '@/i18n';
import { hasErrorSentence } from '@/i18n/errors';

/** The warning in words. */
export function readerWarningText(warning: ReaderWarning, t: TFunction = i18n.t, locale: string = currentLocale()): string {
  const key = `errors.${warning.code}`;
  if (!hasErrorSentence(key, locale)) return warning.message;
  const params = warning.params;
  // The runtime leaf check also covers newer server codes that the generated key union cannot name.
  const sentence: unknown = t(key as TranslationKey, { ...params, returnObjects: false, defaultValue: warning.message });
  return typeof sentence === 'string' ? sentence : warning.message;
}
