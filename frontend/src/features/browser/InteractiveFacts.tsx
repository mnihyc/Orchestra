/** Browser and display facts use their one subject opening, with no ticket represented as a driving lease. */
import { useTranslation } from 'react-i18next';
import { choiceCopy } from '@/i18n/choices';
import type { Browser, Display, JobWaitTime } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

const ENDS = ['quit', 'closed_by_agent', 'idle', 'exited', 'environment_stopped', 'link_lost', 'switched_off', 'revoked', 'protocol'] as const;
export function InteractiveFacts({ subject }: { subject: Browser | Display }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const facts = subject.lifecycle;
  const boundary = (value?: JobWaitTime) => value === undefined || value.state === 'unreported' ? null
    : value.state === 'not_applicable' ? t('placeFacts.notApplicable') : <RelativeTime at={value.at} />;
  const reason = facts?.retirement_reason === 'closed_by_user' ? 'quit' : facts?.retirement_reason;
  const source = facts?.policy_source;
  const policySource = source === 'system' || source === 'default' ? <SourceSettingLink source={{ scope: source, node: null }}
    def={{ key: 'browser.idle_minutes', class: 'policy' }} fallback={t('factDisclosure.notReported')} />
    : source === 'org' ? t('placeFacts.policyOrganization') : source === 'project' ? t('placeFacts.policyProject')
      : source === 'topic' ? t('placeFacts.policyTopic') : source === 'thread' ? t('placeFacts.policyThread') : source === 'environment' ? t('interactiveHistory.environmentPolicy') : null;
  const groups: FactGroup[] = [{ key: 'interactive-current', kind: 'current_setting', facts: [
    { key: 'ownership', label: t('interactiveHistory.ownership'), value: t(subject.origin === 'started' ? 'interactiveHistory.started' : 'interactiveHistory.announced') },
    { key: 'mode', label: t('interactiveHistory.mode'), value: 'preset' in subject ? subject.preset === null ? null : choiceCopy(t, `browserMode.${subject.preset}`).name : t('placeFacts.notApplicable') },
    { key: 'idle', label: t('interactiveHistory.idle'), value: boundary(facts?.idle_close_eligible_at) },
    { key: 'policy', label: t('interactiveHistory.idlePolicy'), value: facts?.idle_minutes === null ? t('placeFacts.notApplicable') : facts?.idle_minutes === undefined ? null
      : facts.idle_minutes === 0 ? t('interactiveHistory.idleOff') : formatSpan(facts.idle_minutes * 60_000, format), source: policySource },
    { key: 'gate', label: t('interactiveHistory.gate'), value: facts === null || facts === undefined ? null : facts.gate === null ? t('placeFacts.notApplicable') : t(`interactiveHistory.gates.${facts.gate}`) },
    { key: 'driving', label: t('interactiveHistory.drivingExpiry'), value: boundary(facts?.driving_idle_expires_at) },
    { key: 'at', label: t('placeFacts.assessed'), value: facts === null || facts === undefined ? null : <RelativeTime at={facts.evaluated_at} /> },
  ] }, { key: 'interactive-recorded', kind: 'recorded_outcome', facts: [
    { key: 'closed', label: t('interactiveHistory.closedAt'), value: subject.closed_at === null ? subject.state === 'closed' ? null : t('placeFacts.notApplicable') : <RelativeTime at={subject.closed_at} /> },
    { key: 'expiry', label: t('interactiveHistory.entryExpiry'), value: boundary(facts?.closed_entry_expires_at) },
    { key: 'reason', label: t('interactiveHistory.retired'), value: reason === 'error' ? t('interactiveHistory.failed') : typeof reason === 'string' && ENDS.some((known) => known === reason)
      ? 'preset' in subject && reason !== 'protocol' ? t(`browser.closed.${reason}`) : t(`vnc.ended.${reason}`) : reason === null && subject.state !== 'closed' ? t('placeFacts.notApplicable') : null },
  ] }];
  return <FactDisclosure groups={groups} />;
}
