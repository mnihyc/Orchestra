/** The existing error alert states a repeated failure once, with its attempt count. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { ErrorAlert } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';
import { ApiError } from '@/api/client';
import { TextPreview } from '@/ui/Preview';
import aliases from '@/i18n/error-aliases.json';
import { RelativeTime } from '@/ui/RelativeTime';

/** A turn's failure with the code internal is its agent program's (an answer of Orch's API that says internal is the server's):
 * it reads as the program's failure and keeps the program's own words, as every other failure of a program does. */
function programFailure(error: unknown): unknown {
  return !(error instanceof ApiError) && typeof error === 'object' && error !== null && 'code' in error && error.code === 'internal'
    ? { ...error, code: 'harness_error' } : error;
}

export function FailureAlert({ error: reported, attempts = 1, words = [], action }: { error: unknown; attempts?: number; words?: readonly string[]; action?: ReactNode }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const say = useErrorText();
  const error = programFailure(reported);
  const text = say(error);
  const message = attempts > 1 ? `${text.message} · ${t('calls.failureAttempts', { count: attempts, attempts: formatNumber(attempts, format.locale) })}` : undefined;
  const alias = Object.hasOwn(aliases,text.code) ? aliases[text.code as keyof typeof aliases] : null;
  const own = !(error instanceof ApiError) && alias?.startsWith('harnessFailures.') === true
    && typeof error === 'object' && error !== null && 'message' in error && typeof error.message === 'string'
    && error.message.trim() !== '' && error.message.trim() !== text.message.trim() ? error.message : null;
  // What the program itself said of this failure, once: its message here and what its own error rows reported.
  const said = [...new Set([own, ...words].filter((line): line is string => line !== null && line.trim() !== ''))];
  const native = said.length === 0 ? null : said.join('\n');
  const resetsAt = typeof error === 'object' && error !== null && 'resets_at' in error && typeof error.resets_at === 'string' ? error.resets_at : null;
  const reportedWindow = typeof error === 'object' && error !== null && 'code' in error && error.code === 'image_generation_limit'
    && 'window' in error && typeof error.window === 'string' ? error.window : null;
  return <div className="flex min-w-0 flex-col gap-1">
    <ErrorAlert error={error} message={message} action={action} />
    {native !== null && <div role="note" aria-label={t('harnessFailures.programMessage')} data-content-role="source">
      <TextPreview value={native} className="font-sans text-sm text-text-muted" />
    </div>}
    {reportedWindow !== null && <p className="text-sm text-text-muted">{t('timeline.failure.reportedLimit', { limit: reportedWindow })}</p>}
    {resetsAt !== null && <p className="text-sm text-text-muted">{t('timeline.failure.resets')} <RelativeTime at={resetsAt} /></p>}
  </div>;
}
