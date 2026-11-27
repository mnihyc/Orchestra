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

/** A program that ended says how it ended and the last lines it wrote, which its runner joins with " | ": a line each here. */
const ENDED = /^(?:[a-z_]+: )?(exited with status -?\d+|was ended by signal \d+): ([\s\S]+)$/;

export function programLines(text: string): string {
  const ended = ENDED.exec(text.trim());
  if (ended === null) return text;
  const lines = (ended[2] ?? '').split('|').map((line) => line.trim()).filter((line) => line !== '');
  return [ended[1] ?? '', ...lines].join('\n');
}

export function FailureAlert({ error, attempts = 1, words = [], action }: { error: unknown; attempts?: number; words?: readonly string[]; action?: ReactNode }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const say = useErrorText();
  const text = say(error);
  const message = attempts > 1 ? `${text.message} · ${t('calls.failureAttempts', { count: attempts, attempts: formatNumber(attempts, format.locale) })}` : undefined;
  const alias = Object.hasOwn(aliases,text.code) ? aliases[text.code as keyof typeof aliases] : null;
  const own = !(error instanceof ApiError) && alias?.startsWith('harnessFailures.') === true
    && typeof error === 'object' && error !== null && 'message' in error && typeof error.message === 'string'
    && error.message.trim() !== '' && error.message.trim() !== text.message.trim() ? error.message : null;
  // What the program itself said of this failure, once: its message here and what its own error rows reported.
  const said = [...new Set([own, ...words].filter((line): line is string => line !== null && line.trim() !== '').map(programLines))];
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
