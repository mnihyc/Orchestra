/** A readable paragraph, its supplied state and explicit refresh/Ask actions. */
import { MessageSquareText, RefreshCw } from 'lucide-react';
import { type ReactNode, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { StatusDot, type DotTone } from '@/ui/StatusDot';
import { cx } from '@/ui/cx';
import { summaryActionKey, type SummaryAskContext, type SummarySnapshot, type SummaryState, type SummaryTarget } from './model';
import { SummaryText } from './SummaryText';

export interface SummaryBoxProps {
  /** Non-secret current reader/view identity; never a credential or authority proof. */
  readerKey: string;
  target: SummaryTarget;
  summary: SummarySnapshot;
  /** Resolves after the page adapter has adopted the server's accepted snapshot. */
  onRefresh?: (() => Promise<void>) | undefined;
  refreshBlockedReason?: string | undefined;
  /** What the box covers has nothing in it (no unread message), so that nothing is asked of a model. */
  nothingNew?: boolean | undefined;
  onConfigure?: (() => void) | undefined;
  /** Opens Ask with this context; opening does not send a paid question. */
  onAsk: (context: SummaryAskContext) => void;
  askBlockedReason?: string | undefined;
  details?: ReactNode;
  className?: string;
}

/** What stopped a summary, as the server names it; each has its sentence under `summaryBox.failure`. */
const failureReasons = ['asker_ended', 'source_gone', 'source_unreadable', 'nothing_new', 'no_summarizer', 'provider_gone', 'provider_not_ready', 'no_interface',
  'model_off', 'model_not_allowed', 'provider_forbidden', 'not_allowed', 'effort_not_offered', 'no_keeper', 'keeper_offline', 'cap_no_room',
  'output_cut', 'model_failed', 'unusable_answer', 'unexpected'] as const;
type FailureReason = (typeof failureReasons)[number];

const failureReason = (error: unknown): FailureReason | null => {
  if (!(error instanceof ApiError)) return null;
  const reason = error.problem.reason;
  return failureReasons.find((known) => known === reason) ?? null;
};

/** The refusal of a refresh that had nothing to summarize, which the server answers before it makes a request. */
const saidNothingNew = (error: unknown): boolean => error instanceof ApiError && error.problem.code === 'summary.nothing_new';

const tones: Record<SummaryState, DotTone> = { empty: 'idle', fresh: 'success', stale: 'idle', scheduled: 'idle', updating: 'info', failed: 'danger', no_summarizer: 'idle' };

/** Reader and page identity own transient request state, never another view's result. */
export function SummaryBox(props: SummaryBoxProps) {
  return <SummaryContent key={JSON.stringify([props.readerKey, props.target.kind, props.target.reference])} {...props} />;
}

function SummaryContent({ target, summary, onRefresh, refreshBlockedReason, nothingNew = false, onConfigure, onAsk, askBlockedReason, details, className }: SummaryBoxProps) {
  const { t } = useTranslation();
  const heading = useId();
  const title = t(target.kind === 'chat_title' ? 'summaryOptions.fields.chat_title' : target.kind === 'chat' ? 'summaryBox.catchUp' : 'summaryBox.title');
  const emptyText = t(target.kind === 'chat' ? 'summaryBox.nothingNew.chat' : 'summaryBox.nothingNew.other');
  const stateText = (text: string) => target.kind === 'chat' || target.kind === 'chat_title'
    ? t('summaryBox.subjectState', { subject: title, state: text }) : text;
  const locked = useRef(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<{ revision: string; error: unknown } | null>(null);
  // A later durable revision supersedes an older request's local error.
  const localError = failure?.revision === summary.revision ? failure : null;
  const hasText = summary.text !== null && summary.text.trim() !== '';
  const shown: SummaryState = pending || summary.state === 'updating' ? 'updating' : localError !== null ? 'failed' : summary.state === 'fresh' && !hasText ? 'empty' : summary.state;
  const error = localError === null ? summary.error : localError.error;
  const reason = failureReason(error);
  // A source with nothing in it is no failure: the box says so plainly, beside any paragraph it already holds.
  const nothingToSay = nothingNew || reason === 'nothing_new' || saidNothingNew(error);
  const state: SummaryState = nothingToSay && shown === 'failed' ? (hasText ? 'fresh' : 'empty') : shown;
  // Say what stopped it, and that a paragraph already written stays.
  const failureText = reason === null ? undefined : hasText ? `${t(`summaryBox.failure.${reason}`)} ${t('summaryBox.failure.kept')}` : t(`summaryBox.failure.${reason}`);
  const refreshable = onRefresh !== undefined && summary.producer?.kind !== 'written' && summary.state !== 'no_summarizer';

  const refresh = async () => {
    if (!refreshable || refreshBlockedReason !== undefined || nothingNew || state === 'updating' || locked.current) return;
    locked.current = true;
    setFailure(null);
    setPending(true);
    try {
      await onRefresh();
    } catch (requestError) {
      setFailure({ revision: summary.revision, error: requestError });
    } finally {
      locked.current = false;
      setPending(false);
    }
  };

  return <section aria-labelledby={heading} aria-busy={state === 'updating'} data-summary-state={state} data-summary-target={target.kind} data-summary-reference={target.reference}
    className={cx('flex min-w-0 flex-col gap-2', className)}>
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <h2 id={heading} className="text-sm font-medium text-text">{title}</h2>
      {state !== 'no_summarizer' && state !== 'empty' && <span role="status"><StatusDot tone={tones[state]} label={stateText(t(`summaryBox.state.${state}`))} showLabel /></span>}
      {summary.producer?.kind === 'model' ? <ResourceChip subject={summary.producer} hint={false} />
        : summary.producer?.kind === 'extension' ? <span className="min-w-0 truncate text-sm text-text-muted" data-overflow-text={summary.producer.name}>{summary.producer.name}</span>
          : summary.producer?.kind === 'written' ? <span className="text-sm text-text-muted">{t('summaryBox.written')}</span> : null}
      {summary.updated_at !== null && <RelativeTime at={summary.updated_at} className="text-sm text-text-muted" />}
      {summary.wait !== undefined && summary.wait !== null && <span className="flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted" data-summary-wait>{t(`summaryBox.wait.${summary.wait.reason}`)}{summary.wait.until !== null && <RelativeTime at={summary.wait.until} />}</span>}
      <div className="ml-auto flex shrink-0 flex-wrap items-center gap-1">
        {details}
        {refreshable && <IconButton size="sm" label={t(summaryActionKey(target.kind, 'refresh'))} icon={<RefreshCw aria-hidden />}
          pending={state === 'updating'} blocked={refreshBlockedReason !== undefined || nothingNew} reason={refreshBlockedReason ?? (nothingNew ? emptyText : undefined)}
          onClick={() => { void refresh(); }} />}
        <Button size="sm" variant="ghost" icon={<MessageSquareText aria-hidden />} blocked={askBlockedReason !== undefined} reason={askBlockedReason} onClick={() => { if (askBlockedReason === undefined) onAsk({ target, summary: summary.text }); }}>{t(summaryActionKey(target.kind, 'ask'))}</Button>
      </div>
    </div>
    <SummaryText text={summary.text} />
    {state === 'updating' && !hasText && <SkeletonRows rows={2} />}
    {state === 'no_summarizer' && <StateLine compact role="status" title={stateText(t('summaryBox.state.no_summarizer'))}
      message={refreshBlockedReason}
      action={onConfigure === undefined ? undefined : <Button size="sm" variant="ghost" onClick={onConfigure}>{t(summaryActionKey(target.kind, 'choose'))}</Button>} />}
    {state === 'failed' && <ErrorAlert error={error} message={failureText} />}
    {state === 'empty' && !hasText && <StateLine compact title={stateText(nothingToSay ? emptyText : t('summaryBox.empty'))} />}
  </section>;
}
