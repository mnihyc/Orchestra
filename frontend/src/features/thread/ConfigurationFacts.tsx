/** The session that accepted setup, and the native comparison with a pending request. */
import { useTranslation } from 'react-i18next';
import type { ConfigurationApplication } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { FactDisclosure, FactDisclosureBody, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function ConfigurationFacts({ application, inline = false }: { application: ConfigurationApplication | null | undefined; inline?: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const pending = application?.pending_kinds;
  const groups: FactGroup[] = [{ key: 'configuration-application', kind: 'recorded_outcome', facts: [
    { key: 'revision', label: t('placeFacts.acceptedRevision'), value: application?.accepted_session_revision === null || application?.accepted_session_revision === undefined ? null : formatNumber(application.accepted_session_revision, locale) },
    { key: 'accepted', label: t('placeFacts.currentSession'), value: application?.accepted_at === null || application?.accepted_at === undefined ? null : <RelativeTime at={application.accepted_at} /> },
    { key: 'pending', label: t('placeFacts.pending'), value: pending === null || pending === undefined ? null : pending.length === 0 ? t('placeFacts.noPendingSetup') : pending.map((kind) => t(`placeFacts.configurationKinds.${kind}`)).join(' · ') },
    { key: 'reload', label: t('placeFacts.reload'), value: application === null || application === undefined ? null : t(`placeFacts.reloadPolicies.${application.reload_policy}`) },
    { key: 'background', label: t('placeFacts.refresh'), value: application === null || application === undefined ? null : application.deferred_by_background ? t('placeFacts.backgroundDefers') : t('placeFacts.notDeferred') },
    { key: 'observed', label: t('placeFacts.observed'), value: application === null || application === undefined ? null : <RelativeTime at={application.observed_at} /> },
  ] }];
  if (inline) return <FactDisclosureBody groups={groups} />;
  const fragment = application?.deferred_by_background === true ? t('placeFacts.backgroundDefers') : pending !== null && pending !== undefined && pending.length > 0 ? t('placeFacts.nextSession') : undefined;
  return <FactDisclosure groups={groups} fragment={fragment} />;
}
