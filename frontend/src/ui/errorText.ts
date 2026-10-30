/** An error in words: what happened and what to do, in the reader's language, with what an administrator needs to find it. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import type { TranslationKey } from '@/i18n';
import { apiErrorSentence, hasErrorSentence } from '@/i18n/errors';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import { formatTimeOfDay } from '@/lib/time';
import nativeAliases from '@/i18n/error-aliases.json';

export interface ErrorText {
  /** What happened and what to do, as one or two sentences. */
  message: string;
  /** The stable code of the error, for the "Details": the API's own, or a name for what the page met. */
  code: string;
  /** The id of the request, by which the server's logs find it. */
  requestId: string | null;
  /** The HTTP status, where something answered. */
  status: number | null;
  /** Whether asking again could help. */
  retryable: boolean;
}

const FAILURE_WORDS: Readonly<Record<string, string | undefined>> = nativeAliases;

function sentence(code: string, t: TFunction, locale: string): string | null {
  const alias = Object.hasOwn(FAILURE_WORDS, code) ? FAILURE_WORDS[code] : undefined;
  if (alias !== undefined && hasErrorSentence(alias,locale)) return t(alias as TranslationKey);
  const key = `errors.${code}`;
  return hasErrorSentence(key,locale) ? t(key as TranslationKey) : null;
}

/** Puts an error into words. `t` is the catalogue; the time of a "try again at" is said in `context`. */
export function errorText(
  error: unknown,
  t: TFunction,
  context: { locale: string; timeZone?: string | undefined },
): ErrorText {
  if (error instanceof ApiError) {
    const at = error.retryAt === null ? null : formatTimeOfDay(error.retryAt, context);
    const message =
      error.code === 'rate_limited' && at !== null ? t('errorsAt.rate_limited', { time: at }) : apiErrorSentence(error.code, error.status, t, context.locale);
    return {
      message,
      code: error.code,
      requestId: error.requestId === '' ? null : error.requestId,
      status: error.status,
      // An answer of the API is an answer; only "not now" is worth asking again.
      retryable: error.status === 429 || error.status >= 500,
    };
  }
  if (error instanceof NetworkError) {
    return {
      message: t(`network.${error.failure}`),
      code: `network.${error.failure}`,
      requestId: null,
      status: error.status,
      retryable: error.transient,
    };
  }
  if (typeof error === 'object' && error !== null && 'code' in error && typeof error.code === 'string') {
    const mapped = sentence(error.code, t, context.locale);
    // Native programs and jobs retain their diagnostics. A newer code still gets translated
    // recovery copy rather than an unreviewed program sentence in the wrong language.
    return { message: mapped ?? t('ui.error.operationFailed'), code: error.code, requestId: null, status: null,
      retryable: /timeout|network|offline|rate|overload/u.test(error.code) };
  }
  if (typeof error === 'string') return { message: sentence(error.trim(), t, context.locale) ?? t('ui.error.operationFailed'), code: 'operation', requestId: null, status: null, retryable: false };
  return { message: t('errors.page'), code: 'page', requestId: null, status: null, retryable: true };
}

/** `errorText` with the catalogue and the reader's time zone at hand. */
export function useErrorText(): (error: unknown) => ErrorText {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  return (error) => errorText(error, t, { locale, timeZone });
}
