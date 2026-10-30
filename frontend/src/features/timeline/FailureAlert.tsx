/** The existing error alert states a repeated failure once, with its attempt count. */
import { useTranslation } from 'react-i18next';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { ErrorAlert } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';

export function FailureAlert({ error, attempts = 1 }: { error: unknown; attempts?: number }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const say = useErrorText();
  const message = attempts > 1 ? `${say(error).message} · ${t('calls.failureAttempts', { count: attempts, attempts: formatNumber(attempts, format.locale) })}` : undefined;
  return <ErrorAlert error={error} message={message} />;
}
