/** Native physical cache observations do not describe erasure of the readable conversation. */
import { createContext, useContext } from 'react';
import { useTranslation } from 'react-i18next';
import type { NativeHistoryLifetime } from '@/generated/api-types';
import { useThreadPart } from '@/realtime/stores/thread';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { FactDisclosure, FactDisclosureBody, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export const NativeHistoryInline = createContext(false);
export function NativeHistoryFacts({ lifetime, inline = false, fragment = true }: { lifetime: NativeHistoryLifetime | null | undefined; inline?: boolean; fragment?: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const policy = lifetime?.policy_source;
  const source = policy === 'default' || policy === 'system' ? <SourceSettingLink source={{ scope: policy, node: null }}
    def={{ key: 'retention.agent_history_idle_days', class: 'policy' }} fallback={t('factDisclosure.notReported')} />
    : policy === 'org' ? t('placeFacts.policyOrganization') : policy === 'project' ? t('placeFacts.policyProject')
      : policy === 'topic' ? t('placeFacts.policyTopic') : policy === 'thread' ? t('placeFacts.policyThread') : null;
  const due = lifetime?.idle_eligible_at;
  const groups: FactGroup[] = [{ key: 'native-history', title: t('placeFacts.nativeHistory'), facts: [
    { key: 'state', label: t('placeFacts.lastNativeState'), value: lifetime === null || lifetime === undefined || lifetime.state === 'unreported' ? null : t(`placeFacts.nativeHistoryStates.${lifetime.state}`) },
    { key: 'due', label: t('placeFacts.nativeIdleEligible'), value: due === undefined || due.state === 'unreported' ? null : due.state === 'not_applicable' ? lifetime?.policy_days === 0 ? t('placeFacts.nativeCleanupOff') : t('placeFacts.notApplicable') : <RelativeTime at={due.at} /> },
    { key: 'policy', label: t('placeFacts.nativeRetention'), value: lifetime?.policy_days === null || lifetime?.policy_days === undefined ? null : lifetime.policy_days === 0 ? t('placeFacts.nativeCleanupOff') : t('placeFacts.retentionDays', { count: lifetime.policy_days, n: formatNumber(lifetime.policy_days, locale) }), source },
    { key: 'policy-at', label: t('placeFacts.assessed'), value: lifetime?.policy_observed_at === null || lifetime?.policy_observed_at === undefined ? null : <RelativeTime at={lifetime.policy_observed_at} /> },
    { key: 'removed', label: t('placeFacts.nativeRemoved'), value: lifetime?.removed_at === null || lifetime?.removed_at === undefined ? lifetime?.state === 'retained' ? t('placeFacts.notApplicable') : null : <RelativeTime at={lifetime.removed_at} /> },
    { key: 'observed', label: t('placeFacts.observed'), value: lifetime?.observed_at === null || lifetime?.observed_at === undefined ? null : <RelativeTime at={lifetime.observed_at} /> },
  ] }];
  if (inline) return <div className="w-full min-w-0 basis-full text-left"><FactDisclosureBody groups={groups} /></div>;
  return <FactDisclosure groups={groups} fragment={fragment && lifetime?.state === 'removed' ? t('placeFacts.nativeHistoryStates.removed') : undefined} />;
}
export function SessionNativeHistory({ threadId, segmentId, fragment = true }: { threadId: string; segmentId: string; fragment?: boolean }) {
  const inline = useContext(NativeHistoryInline);
  const lifetime = useThreadPart(threadId, (view) => view.segments.find((segment) => segment.id === segmentId)?.native_history);
  return <NativeHistoryFacts lifetime={lifetime} inline={inline} fragment={fragment} />;
}
