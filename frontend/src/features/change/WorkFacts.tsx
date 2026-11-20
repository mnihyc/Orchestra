/** Count assessments and execution budgets captured by their native suppliers. */
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { Job, JobWaitTime, JobWorkDeadline, JobWorkPolicy, Measurement, MeasurementTiming } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan, type TimeContext } from '@/lib/time';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

function policySource(policy: JobWorkPolicy | null | undefined, t: TFunction, locale: string): ReactNode {
  if (policy === null || policy === undefined) return null;
  return <><SourceSettingLink source={{ scope: policy.origin, node: null }} def={{ key: policy.key, class: 'policy' }} fallback={t('factDisclosure.notReported')} />
    {policy.revision !== null && <span className="text-text-muted">{t('placeFacts.policyVersion', { n: formatNumber(policy.revision, locale) })}</span>}</>;
}
function moment(value: JobWaitTime | undefined, t: TFunction): ReactNode {
  return value === undefined || value.state === 'unreported' ? null : value.state === 'not_applicable' ? t('placeFacts.notApplicable') : <RelativeTime at={value.at} />;
}
export function measurementTimingGroup(timing: MeasurementTiming | null | undefined, t: TFunction, locale: string): FactGroup {
  return { key: 'count-assessment', title: t('placeFacts.countAssessment'), facts: [
    { key: 'cache', label: t('placeFacts.cacheValidUntil'), value: moment(timing?.cache_valid_until, t), source: policySource(timing?.cache_policy, t, locale) },
    { key: 'assessed', label: t('placeFacts.assessed'), value: timing === null || timing === undefined ? null : <RelativeTime at={timing.assessed_at} /> },
  ] };
}
export function measurementObserverGroup(timing: MeasurementTiming | null | undefined, t: TFunction, locale: string): FactGroup {
  return { key: 'count-observer', title: t('placeFacts.countLifetime'), facts: [
    { key: 'grace', label: t('placeFacts.observerGraceUntil'), value: moment(timing?.observer_grace_until, t), source: policySource(timing?.observer_policy, t, locale) },
    { key: 'assessed', label: t('placeFacts.observed'), value: timing === null || timing === undefined ? null : <RelativeTime at={timing.assessed_at} /> },
  ] };
}
export function workDeadlineGroup(deadline: JobWorkDeadline | null | undefined, t: TFunction, format: TimeContext): FactGroup {
  return { key: 'execution-budget', title: t('placeFacts.executionBudget'), facts: [
    { key: 'accepted', label: t('placeFacts.executionAccepted'), value: deadline === null || deadline === undefined ? null : <RelativeTime at={deadline.accepted_at} /> },
    { key: 'until', label: t('placeFacts.executionDeadline'), value: deadline === null || deadline === undefined ? null : <RelativeTime at={deadline.at} /> },
    { key: 'allowance', label: t('placeFacts.workAllowance'), value: deadline === null || deadline === undefined ? null : formatSpan(deadline.seconds * 1_000, format), source: policySource(deadline?.policy, t, format.locale) },
  ] };
}
export function MeasurementFacts({ reference, job }: { reference: Measurement; job: Job | undefined }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  return <FactDisclosure groups={[measurementTimingGroup(reference.timing, t, format.locale),
    measurementObserverGroup(job?.measurement_timing ?? reference.timing, t, format.locale),
    job === undefined ? null : workDeadlineGroup(job.progress.activity?.captured_deadline, t, format)]} />;
}
