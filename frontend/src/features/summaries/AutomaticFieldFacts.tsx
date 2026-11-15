/** An automatic field's retained outcome lives in its own existing menu, with its real schedule. */
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { Node } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { FactDisclosure, FactDisclosureBody, type FactDisclosureBodyProps, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';
import { formatSpan } from '@/lib/time';
import { conformanceFacts, conformanceFragment, ProducerFact, usageWords } from './facts';

type Subject = Pick<Node, 'id' | 'kind' | 'name' | 'path' | 'automatic_fields' | 'title_source' | 'tags_source'>;
type Field = 'title' | 'tags';

export function automaticFieldFragment(node: Subject, field: Field, t: TFunction): ReactNode {
  const facts = node.automatic_fields?.[field];
  const schedule = facts?.schedule;
  if (schedule?.state === 'queued' && schedule.due_at === null) return t('summaryBox.state.updating');
  if (schedule?.state === 'queued' || schedule?.state === 'retrying') {
    const at = schedule.due_at === null ? schedule.next_at : schedule.next_at !== null && schedule.next_at > schedule.due_at ? schedule.next_at : schedule.due_at;
    return <>{t(schedule.state === 'retrying' ? 'summaryBox.wait.recovery' : 'summaryBox.wait.interval')}{at !== null && <> <RelativeTime at={at} /></>}</>;
  }
  if (facts?.last_attempt?.outcome === 'discarded') return t('summaryFacts.resultDiscarded');
  if (facts?.last_attempt?.outcome === 'unusable') return t('summaryFacts.answerDiscarded');
  if (schedule?.state === 'skipped') return t('summaryFacts.skipped', { reason: schedule.reason === null ? t('factDisclosure.notReported') : t(`summaryFacts.skip.${schedule.reason}`) });
  return conformanceFragment(facts?.last_attempt?.result_facts, t);
}

export function automaticFieldBody(node: Subject, field: Field, t: TFunction, locale: string): FactDisclosureBodyProps | null {
  const captured = node.automatic_fields?.[field];
  const automatic = (field === 'title' ? node.title_source : node.tags_source) === 'auto';
  if ((captured === null || captured === undefined) && !automatic) return null;
  const attempt = captured?.last_attempt;
  const schedule = captured?.schedule;
  const self = node.path.find((place) => place.id === node.id && !place.redacted && place.name !== null);
  const groups: FactGroup[] = [{ key: `${field}-outcome`, kind: 'recorded_outcome', facts: [
    { key: 'producer', label: t('summaryFacts.producer'), value: <ProducerFact view={attempt?.producer} /> },
    ...(attempt === null || attempt === undefined ? [] : [{ key: 'outcome', label: t('summaryFacts.outcome'), value: t(`summaryFacts.outcomes.${attempt.outcome}`) }]),
    ...(attempt?.outcome !== 'discarded' ? [] : [{ key: 'discarded', label: t('summaryFacts.discarded'), value: attempt.discard_reason === null ? undefined : t(`summaryFacts.discard.${attempt.discard_reason}`) }]),
    ...(attempt?.usage_owner === null || attempt?.usage_owner === undefined ? [] : [{ key: 'owner', label: t('summaryFacts.usageOwner'), value: <UserChip user={attempt.usage_owner} /> }]),
    { key: 'usage', label: t('summaryFacts.recordedUsage'), value: usageWords(attempt?.captured_usage, t, locale) },
    ...conformanceFacts(attempt?.result_facts, t, locale).filter((fact) => fact.key !== 'usage'),
    { key: 'attempt', label: t('summaryFacts.lastAttempt'), value: attempt === null || attempt === undefined ? undefined : <RelativeTime at={attempt.at} /> },
    { key: 'success', label: t('summaryFacts.lastUpdated'), value: schedule?.last_success_at === null || schedule?.last_success_at === undefined ? undefined : <RelativeTime at={schedule.last_success_at} /> },

    { key: 'due', label: t('summaryBox.wait.interval'), value: schedule?.due_at === null || schedule?.due_at === undefined ? undefined : <RelativeTime at={schedule.due_at} /> },
    { key: 'retry', label: t('summaryBox.wait.recovery'), value: schedule?.next_at === null || schedule?.next_at === undefined ? undefined : <RelativeTime at={schedule.next_at} /> },
    { key: 'ceiling', label: t('summaryFacts.retryCeiling'), value: schedule?.retry_ceiling === null || schedule?.retry_ceiling === undefined ? undefined : formatSpan(schedule.retry_ceiling * 1000, { locale }, 'short'),
      source: schedule?.source === null || schedule?.source === undefined ? undefined : <SourceSettingLink source={schedule.source} def={{ key: 'summary.retry_ceiling_secs', class: 'parameter' }} target={{ kind: 'node', id: node.id }} fallback={t('summaryFacts.notShared')} defaultTarget={self} {...(schedule.source.scope === 'default' ? { text: t('settings.editor.from.default') } : {})} /> },
  ] }];
  return { groups };
}

export function AutomaticFieldFacts({ node, field, inline = false }: { node: Subject; field: Field; inline?: boolean }) {
  const { t, i18n } = useTranslation();
  const body = automaticFieldBody(node, field, t, i18n.resolvedLanguage ?? 'en');
  if (body === null) return null;
  return inline ? <FactDisclosureBody {...body} /> : <FactDisclosure {...body} label={t(`summaryFacts.${field}`)} fragment={automaticFieldFragment(node, field, t)} />;
}
