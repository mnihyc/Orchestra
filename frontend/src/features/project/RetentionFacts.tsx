/** Native automatic eligibility and physical holds stay separate from a frozen restoration cutoff. */
import { useTranslation } from 'react-i18next';
import type { NodeCleanupHold, NodeRetention, SettingScope } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function RetentionFacts({ retention, purgeAt, cleanup }: {
  retention?: NodeRetention | null; purgeAt?: string; cleanup?: NodeCleanupHold | null;
}) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const source = (scope: SettingScope | null | undefined, key: string) => scope === 'default' || scope === 'system'
    ? <SourceSettingLink source={{ scope, node: null }} def={{ key, class: 'policy' }} fallback={t('factDisclosure.notReported')} />
    : scope === 'org' ? t('placeFacts.policyOrganization') : scope === 'project' ? t('placeFacts.policyProject')
      : scope === 'topic' ? t('placeFacts.policyTopic') : scope === 'thread' ? t('placeFacts.policyThread') : null;
  const groups: FactGroup[] = purgeAt === undefined ? (['archive', 'delete'] as const).map((kind) => {
    const policy = retention?.[kind];
    const due = policy?.eligible_at;
    const off = policy?.days === 0;
    return { key: kind, title: t(`placeFacts.retention.${kind}`), facts: [
      { key: 'policy', label: t('placeFacts.nativeRetention'), value: policy?.days === undefined ? null : policy.days === null ? t('placeFacts.notApplicable') : off
        ? t(`placeFacts.retention.${kind}Off`) : t('placeFacts.retentionDays', { count: policy.days, n: formatNumber(policy.days, locale) }),
      source: source(policy?.source, kind === 'archive' ? 'retention.archive_idle_days' : 'retention.delete_archived_days') },
      { key: 'due', label: t(`placeFacts.retention.${kind}Due`), value: due === undefined || due.state === 'unreported' ? null
        : due.state === 'not_applicable' ? t('placeFacts.notApplicable') : <RelativeTime at={due.at} /> },
      { key: 'hold', label: t('placeFacts.retention.heldBy'), value: policy?.hold_reason === null ? t('placeFacts.notApplicable')
        : policy?.hold_reason === undefined ? null : t(`placeFacts.retention.holds.${policy.hold_reason}`) },
      { key: 'at', label: t('placeFacts.assessed'), value: retention === undefined || retention === null ? null : <RelativeTime at={retention.measured_at} /> },
    ] };
  }) : [{ key: 'cleanup', kind: 'recorded_outcome', facts: [
    { key: 'recovery', label: t('placeFacts.retention.recoveryCutoff'), value: <RelativeTime at={purgeAt} /> },
    { key: 'hold', label: t('placeFacts.retention.physicalCleanup'), value: cleanup === null || cleanup === undefined ? null
      : t(cleanup.held ? 'placeFacts.retention.dependencies' : 'placeFacts.retention.noHold') },
    { key: 'at', label: t('placeFacts.assessed'), value: cleanup === null || cleanup === undefined ? null : <RelativeTime at={cleanup.measured_at} /> },
  ] }];
  return <FactDisclosure groups={groups} />;
}
