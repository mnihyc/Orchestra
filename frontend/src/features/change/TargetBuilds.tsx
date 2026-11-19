/** An update names each target's check, keeping failed and untested builds selectable. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { programBuildsQuery } from '@/api/harnesses';
import { BuildStatus } from '@/features/resources/agents/BuildStatus';
import type { PendingUpdate, RunningHarnessBuild } from '@/generated/api-types';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { programNameIn } from '@/features/resources/programNames';
import { disabledFeatures, featureFallback } from '@/features/resources/agents/buildChecks';

export function TargetBuilds({ updates, programs }: { updates: readonly PendingUpdate[]; programs?: readonly RunningHarnessBuild[] | null }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const targets = updates.filter((change) => change.what === 'harness');
  const builds = useQuery({ ...programBuildsQuery, enabled: targets.length > 0 && programs === undefined });
  if (targets.length === 0) return null;
  if (programs === undefined && builds.isPending) return <SkeletonRows rows={1} />;
  if (programs === undefined && builds.isError) return <ErrorAlert error={builds.error} />;
  return <div className="flex flex-col gap-1">{targets.map((target) => {
    const build = programs === undefined ? builds.data?.find((build) => build.kind === target.name && build.version === target.to) : programs?.find((program) => program.kind === target.name && program.version === target.to)?.build ?? undefined;
    const off = build === undefined ? [] : disabledFeatures(build);
    const name = programNameIn(target, locale) ?? programNameIn(programs?.find((program) => program.kind === target.name), locale) ?? programNameIn(build, locale) ?? t('resources.harnessNameUnknown');
    return <div key={`${target.name}:${target.to ?? ''}`} className="flex min-w-0 flex-col gap-1 text-sm">
      <div className="flex min-w-0 flex-wrap items-center gap-2"><span className="min-w-0 wrap-anywhere">{name} {target.to}</span>{build !== undefined && <BuildStatus build={build} name={name} />}</div>
      {build !== undefined && off.map((feature) => {
        const fallback = featureFallback(build, feature, t);
        return fallback === null ? null : <span key={feature} className="wrap-anywhere text-text-muted" role="status">{fallback}</span>;
      })}
      {(build === undefined || build.status === 'untested' || build.status === 'failed' && off.length === 0) && <span className="wrap-anywhere text-warning-text" role="status">{t(build === undefined ? 'programVersions.unknownCheck' : build.status === 'untested' ? 'programVersions.notChecked' : 'programVersions.checkFailed')}</span>}
    </div>;
  })}</div>;
}
