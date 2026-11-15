/** The existing error alert states a repeated failure once, with its attempt count. */
import { useTranslation } from 'react-i18next';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { ErrorAlert } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';
import { ApiError } from '@/api/client';
import { TextPreview } from '@/ui/Preview';
import aliases from '@/i18n/error-aliases.json';
import { RelativeTime } from '@/ui/RelativeTime';

export function FailureAlert({ error, attempts = 1 }: { error: unknown; attempts?: number }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const say = useErrorText();
  const text = say(error);
  const message = attempts > 1 ? `${text.message} · ${t('calls.failureAttempts', { count: attempts, attempts: formatNumber(attempts, format.locale) })}` : undefined;
  const alias = Object.hasOwn(aliases,text.code) ? aliases[text.code as keyof typeof aliases] : null;
  const native = !(error instanceof ApiError) && alias?.startsWith('harnessFailures.') === true
    && typeof error === 'object' && error !== null && 'message' in error && typeof error.message === 'string'
    && error.message.trim() !== '' && error.message.trim() !== text.message.trim() ? error.message : null;
  const resetsAt = typeof error === 'object' && error !== null && 'resets_at' in error && typeof error.resets_at === 'string' ? error.resets_at : null;
  const reportedWindow = typeof error === 'object' && error !== null && 'code' in error && error.code === 'image_generation_limit'
    && 'window' in error && typeof error.window === 'string' ? error.window : null;
  return <div className="flex min-w-0 flex-col gap-1">
    <ErrorAlert error={error} message={message} />
    {native !== null && <div role="note" aria-label={t('harnessFailures.programMessage')} data-content-role="source">
      <TextPreview value={native} className="font-sans text-sm text-text-muted" />
    </div>}
    {reportedWindow !== null && <p className="text-sm text-text-muted">{t('timeline.failure.reportedLimit', { limit: reportedWindow })}</p>}
    {resetsAt !== null && <p className="text-sm text-text-muted">{t('timeline.failure.resets')} <RelativeTime at={resetsAt} /></p>}
  </div>;
}
