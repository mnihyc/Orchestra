/** A readable paragraph, its supplied state and explicit refresh/Ask actions. */
import { MessageSquareText, RefreshCw } from 'lucide-react';
import { type ReactNode, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { StatusDot, type DotTone } from '@/ui/StatusDot';
import { cx } from '@/ui/cx';
import type { SummaryAskContext, SummarySnapshot, SummaryState, SummaryTarget } from './model';
import { SummaryText } from './SummaryText';

export interface SummaryBoxProps {
  /** Non-secret current reader/view identity; never a credential or authority proof. */
  readerKey: string;
  target: SummaryTarget;
  summary: SummarySnapshot;
  /** Resolves after the page adapter has adopted the server's accepted snapshot. */
  onRefresh?: (() => Promise<void>) | undefined;
  refreshBlockedReason?: string | undefined;
  onConfigure?: (() => void) | undefined;
  /** Opens Ask with this context; opening does not send a paid question. */
  onAsk: (context: SummaryAskContext) => void;
  askBlockedReason?: string | undefined;
  details?: ReactNode;
  className?: string;
}

const tones: Record<SummaryState, DotTone> = { empty: 'idle', fresh: 'success', stale: 'idle', scheduled: 'idle', updating: 'info', failed: 'danger', no_summarizer: 'idle' };

/** Reader and page identity own transient request state, never another view's result. */
export function SummaryBox(props: SummaryBoxProps) {
  return <SummaryContent key={JSON.stringify([props.readerKey, props.target.kind, props.target.reference])} {...props} />;
}

function SummaryContent({ target, summary, onRefresh, refreshBlockedReason, onConfigure, onAsk, askBlockedReason, details, className }: SummaryBoxProps) {
  const { t } = useTranslation();
  const heading = useId();
  const locked = useRef(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<{ revision: string; error: unknown } | null>(null);
  // A later durable revision supersedes an older request's local error.
  const localError = failure?.revision === summary.revision ? failure : null;
  const hasText = summary.text !== null && summary.text.trim() !== '';
  const state: SummaryState = pending || summary.state === 'updating' ? 'updating' : localError !== null ? 'failed' : summary.state === 'fresh' && !hasText ? 'empty' : summary.state;
  const error = localError === null ? summary.error : localError.error;
  const refreshable = onRefresh !== undefined && summary.producer?.kind !== 'written' && summary.state !== 'no_summarizer';

  const refresh = async () => {
    if (!refreshable || refreshBlockedReason !== undefined || state === 'updating' || locked.current) return;
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
      <h2 id={heading} className="text-sm font-medium text-text">{t(target.kind === 'chat_title' ? 'summaryOptions.fields.chat_title' : target.kind === 'chat' ? 'summaryBox.catchUp' : 'summaryBox.title')}</h2>
      {state !== 'no_summarizer' && state !== 'empty' && <span role="status"><StatusDot tone={tones[state]} label={t(`summaryBox.state.${state}`)} showLabel /></span>}
      {summary.producer?.kind === 'model' ? <ResourceChip subject={summary.producer} hint={false} />
        : summary.producer?.kind === 'extension' ? <span className="min-w-0 truncate text-sm text-text-muted" data-overflow-text={summary.producer.name}>{summary.producer.name}</span>
          : summary.producer?.kind === 'written' ? <span className="text-sm text-text-muted">{t('summaryBox.written')}</span> : null}
      {summary.updated_at !== null && <RelativeTime at={summary.updated_at} className="text-sm text-text-muted" />}
      {summary.wait !== undefined && summary.wait !== null && <span className="flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted" data-summary-wait>{t(`summaryBox.wait.${summary.wait.reason}`)}{summary.wait.until !== null && <RelativeTime at={summary.wait.until} />}</span>}
      <div className="ml-auto flex shrink-0 flex-wrap items-center gap-1">
        {details}
        {refreshable && <IconButton size="sm" label={t('summaryBox.refresh')} icon={<RefreshCw aria-hidden />}
          pending={state === 'updating'} blocked={refreshBlockedReason !== undefined} reason={refreshBlockedReason}
          onClick={() => { void refresh(); }} />}
        <Button size="sm" variant="ghost" icon={<MessageSquareText aria-hidden />} blocked={askBlockedReason !== undefined} reason={askBlockedReason} onClick={() => { if (askBlockedReason === undefined) onAsk({ target, summary: summary.text }); }}>{t('summaryBox.ask')}</Button>
      </div>
    </div>
    <SummaryText text={summary.text} />
    {state === 'updating' && !hasText && <SkeletonRows rows={2} />}
    {state === 'no_summarizer' && <StateLine compact role="status" title={t('summaryBox.state.no_summarizer')}
      message={refreshBlockedReason}
      action={onConfigure === undefined ? undefined : <Button size="sm" variant="ghost" onClick={onConfigure}>{t('summaryBox.choose')}</Button>} />}
    {state === 'failed' && <ErrorAlert error={error} />}
    {state === 'empty' && !hasText && <StateLine compact title={t('summaryBox.empty')} />}
  </section>;
}
