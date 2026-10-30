/** Recovery history uses recorded attempts and a sentence, with one link to its rule. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import type { MarkerCallRecovery } from '@/generated/api-types';
import { formatNumber } from '@/lib/format';
import { useThreadPart } from '@/realtime/stores/thread';
import { useTimelineContext } from '@/features/timeline/context';

type RecordedRecovery = MarkerCallRecovery & { total?: number | null };

/** A causal clause is authored separately from the capitalized status label. */
export function recoveryCause(kind: string, t: TFunction): string {
  return t(`calls.causes.${kind}`, { defaultValue: t('calls.causes.other') });
}

export function recoverySentence(data: RecordedRecovery, t: TFunction, locale: string, includeRule = true): string {
  const action = t(`calls.history.${data.action}`, { defaultValue: t('calls.history.retry'), cause: recoveryCause(data.kind, t) });
  const attempt = t(typeof data.total === 'number' ? 'calls.attempt' : 'calls.attemptOnly', {
    attempt: formatNumber(data.attempt, locale), total: typeof data.total === 'number' ? formatNumber(data.total, locale) : '–',
  });
  return [action, attempt, ...(includeRule ? [t('calls.ruleNamed', { rule: data.rule })] : [])].join(' · ');
}

/** The thread's served topic identifies its inherited rules; opening the link resolves normal access. */
export function RecoveryRuleLink({ rule }: { rule: string }) {
  const { t } = useTranslation();
  const { threadId } = useTimelineContext();
  const topic = useThreadPart(threadId, (view) => view.thread?.topic_id);
  const label = t('calls.ruleNamed', { rule });
  return topic === undefined ? <span>{label}</span> : <a className="focus-inset inline-block max-w-full rounded-sm wrap-anywhere text-accent-text underline underline-offset-2" href={`/topic/${encodeURIComponent(topic)}?tab=settings&section=policies&setting=calls.rules#${encodeURIComponent(rule)}`}>{label}</a>;
}
