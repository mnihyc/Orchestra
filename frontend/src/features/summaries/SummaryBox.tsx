/** A readable paragraph, its supplied state and explicit refresh/Ask actions. */
import { ChevronRight, MessageSquareText, RefreshCw } from 'lucide-react';
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
  /** A summary above a page's content that has nothing yet (nothing written, nothing on its way, nothing to explain) is
   * one closed row with its title and that it has nothing yet, opened by whoever wants its acts. */
  foldWhenEmpty?: boolean | undefined;
  /** The person's own choice to show or put away the summary above a thread: its title shows or folds it. */
  fold?: SummaryFold | undefined;
}

export interface SummaryFold { open: boolean; onOpenChange: (open: boolean) => void }

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

const changeReasons = ['source_changed', 'content_changing', 'settings_changed'] as const;
const changeReason = (error: unknown) => error instanceof ApiError ? changeReasons.find((reason) => reason === error.problem.reason) ?? null : null;

const tones: Record<SummaryState, DotTone> = { empty: 'idle', fresh: 'success', stale: 'idle', scheduled: 'idle', updating: 'info', failed: 'danger', no_summarizer: 'idle' };

/** Reader and page identity own transient request state, never another view's result. */
export function SummaryBox(props: SummaryBoxProps) {
  return <SummaryContent key={JSON.stringify([props.readerKey, props.target.kind, props.target.reference])} {...props} />;
}

function SummaryContent({ target, summary, onRefresh, refreshBlockedReason, nothingNew = false, onConfigure, onAsk, askBlockedReason, details, className, foldWhenEmpty = false, fold: chosen }: SummaryBoxProps) {
  const { t } = useTranslation();
  const heading = useId();
  const body = useId();
  const title = t(target.kind === 'chat_title' ? 'summaryOptions.fields.chat_title' : target.kind === 'chat' ? 'summaryBox.catchUp' : 'summaryBox.title');
  const emptyText = t(target.kind === 'chat' ? 'summaryBox.nothingNew.chat' : 'summaryBox.nothingNew.other');
  const stateText = (text: string) => target.kind === 'chat' || target.kind === 'chat_title'
    ? t('summaryBox.subjectState', { subject: title, state: text }) : text;
  const locked = useRef(false);
  const [emptyOpen, setEmptyOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<{ revision: string; error: unknown } | null>(null);
  // A later durable revision supersedes an older request's local error.
  const localError = failure?.revision === summary.revision ? failure : null;
  const hasText = summary.text !== null && summary.text.trim() !== '';
  const shown: SummaryState = pending || summary.state === 'updating' ? 'updating' : localError !== null ? 'failed' : summary.state === 'fresh' && !hasText ? 'empty' : summary.state;
  const error = localError === null ? summary.error : localError.error;
  const reason = failureReason(error);
  const changed = changeReason(error);
  // A source with nothing in it is no failure: the box says so plainly, beside any paragraph it already holds.
  const nothingToSay = nothingNew || reason === 'nothing_new' || saidNothingNew(error);
  const state: SummaryState = shown === 'updating' ? shown : changed !== null ? hasText ? 'stale' : 'empty' : nothingToSay ? 'empty' : shown;
  // Say what stopped it, and that a paragraph already written stays.
  const failureText = reason === null ? undefined : hasText ? `${t(`summaryBox.failure.${reason}`)} · ${t('summaryBox.failure.kept')}` : t(`summaryBox.failure.${reason}`);
  const changeText = changed === null ? undefined : hasText ? `${t(`summaryBox.changed.${changed}`)} · ${t('summaryBox.failure.kept')}` : t(`summaryBox.changed.${changed}`);
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

  // Nothing written yet, nothing on its way and nothing to explain: one closed row says so, above the page's content,
  // whatever the person chose for a summary with words.
  const empty = state === 'empty' && !hasText && changed === null;
  const fold: SummaryFold | undefined = foldWhenEmpty && empty ? { open: emptyOpen, onOpenChange: setEmptyOpen } : chosen;
  // Folded, the row says what a reader would look for first: the summary's first line, or why there is none.
  const firstLine = hasText ? summary.text?.split('\n').find((line) => line.trim() !== '')?.trim() : undefined;
  const essentials = firstLine ?? (state === 'empty' ? (nothingToSay ? emptyText : t('summaryBox.empty'))
    : changed !== null ? changeText : state === 'failed' && failureText !== undefined ? failureText : stateText(t(`summaryBox.state.${state}`)));
  const toggle = fold === undefined ? null : <SummaryFoldToggle title={title} fold={fold} controls={body} essentials={essentials} />;
  if (fold !== undefined && !fold.open) {
    return <section aria-labelledby={heading} data-summary-fold data-summary-state={state} data-summary-target={target.kind} data-summary-reference={target.reference}
      className={cx('flex min-w-0', className)}>
      <h2 id={heading} className="min-w-0 max-w-full">{toggle}</h2>
    </section>;
  }

  const box = <section id={fold === undefined ? undefined : body} aria-labelledby={heading} aria-busy={state === 'updating'} data-summary-state={state} data-summary-target={target.kind} data-summary-reference={target.reference}
    {...(fold === undefined ? {} : { 'data-summary-fold': 'open' })} className={cx('flex min-w-0 flex-col gap-2', className)}>
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <h2 id={heading} className="text-sm font-medium text-text">{toggle ?? title}</h2>
      {state !== 'no_summarizer' && state !== 'empty' && <span role="status"><StatusDot tone={tones[state]} label={stateText(t(`summaryBox.state.${state}`))} showLabel /></span>}
      {summary.producer?.kind === 'model' ? <ResourceChip subject={summary.producer} hint={false} />
        : summary.producer?.kind === 'extension' ? <span className="min-w-0 truncate text-sm text-text-muted" data-overflow-text={summary.producer.name}>{summary.producer.name}</span>
          : summary.producer?.kind === 'written' ? <span className="text-sm text-text-muted">{t('summaryBox.written')}</span> : null}
      {summary.updated_at !== null && <span className="flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted" data-summary-updated>{t('summaryBox.lastUpdated')}<RelativeTime at={summary.updated_at} /></span>}
      {summary.wait !== undefined && summary.wait !== null && <span className="flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted" data-summary-wait>{t(`summaryBox.wait.${summary.wait.reason}`)}{summary.wait.until !== null && <RelativeTime at={summary.wait.until} />}</span>}
      <div className="ml-auto flex min-w-0 max-w-full flex-wrap items-center gap-1">
        {details}
        {state === 'no_summarizer' && onConfigure !== undefined && <Button size="sm" variant="ghost" onClick={onConfigure}>{t(summaryActionKey(target.kind, 'choose'))}</Button>}
        {refreshable && <IconButton size="sm" label={t(changed === null ? summaryActionKey(target.kind, 'refresh') : 'summaryBox.summarizeAgain')} icon={<RefreshCw aria-hidden />}
          pending={state === 'updating'} blocked={refreshBlockedReason !== undefined || nothingNew} reason={refreshBlockedReason ?? (nothingNew ? emptyText : undefined)}
          onClick={() => { void refresh(); }} />}
        <Button size="sm" variant="ghost" icon={<MessageSquareText aria-hidden />} blocked={askBlockedReason !== undefined} reason={askBlockedReason} onClick={() => { if (askBlockedReason === undefined) onAsk({ target, summary: summary.text }); }}>{t(summaryActionKey(target.kind, 'ask'))}</Button>
      </div>
    </div>
    <SummaryText text={summary.text} />
    {state === 'updating' && !hasText && <SkeletonRows rows={2} />}
    {state === 'failed' && <ErrorAlert error={error} message={failureText} />}
    {changed !== null && state !== 'updating' && <StateLine compact title={changeText} />}
    {changed === null && state === 'empty' && (nothingToSay || !hasText) && <StateLine compact title={nothingToSay ? emptyText : t('summaryBox.empty')} />}
  </section>;
  return box;
}

/** The one row a folded summary is: its title, and while folded what it says first; pressing it opens or folds it. */
function SummaryFoldToggle({ title, fold, controls, essentials }: { title: string; fold: SummaryFold; controls?: string | undefined; essentials?: string | undefined }) {
  const { t } = useTranslation();
  return <button type="button" aria-expanded={fold.open} aria-controls={controls} data-summary-fold-toggle
    className="focus-inset flex min-w-0 max-w-full cursor-default items-center gap-1 rounded-sm text-sm select-none" onClick={() => fold.onOpenChange(!fold.open)}>
    <ChevronRight aria-hidden className={cx('size-4 shrink-0 text-text-subtle transition-transform motion-reduce:transition-none', fold.open && 'rotate-90')} />
    <span className="shrink-0 font-medium text-text">{title}</span>
    {!fold.open && essentials !== undefined && <span className="min-w-0 truncate font-normal text-text-muted" data-overflow-text={essentials}>{t('resourceFacts.separator')}{essentials}</span>}
  </button>;
}

/** A folded summary the person put away themselves: its row alone, with nothing read until it is opened again. */
export function SummaryFoldRow({ fold, className }: { fold: SummaryFold; className?: string | undefined }) {
  const { t } = useTranslation();
  const heading = useId();
  return <section aria-labelledby={heading} data-summary-fold className={cx('flex min-w-0', className)}>
    <h2 id={heading} className="min-w-0 max-w-full"><SummaryFoldToggle title={t('summaryBox.title')} fold={fold} /></h2>
  </section>;
}
