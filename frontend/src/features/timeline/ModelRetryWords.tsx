/** Words for the native retry phase; its reported counter is separate from request-ledger attempts. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';

export function modelRetryWords(nativeAttempt: unknown, t: TFunction, locale: string): string {
  return typeof nativeAttempt === 'number' && Number.isSafeInteger(nativeAttempt) && nativeAttempt > 0
    ? t('timeline.status.retryingAttempt', { attempt: formatNumber(nativeAttempt, locale) })
    : t('timeline.status.retrying');
}

/** Inline words only; the existing Now line owns its layout, elapsed clock and Stop control. */
export function ModelRetryWords({ attempt }: { attempt?: unknown }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return modelRetryWords(attempt, t, locale);
}
