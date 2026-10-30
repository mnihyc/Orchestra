/** The same actual build and disabled-feature facts wherever an agent program is shown. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { environmentProgramsQuery } from '@/api/harnesses';
import { BuildStatus } from '@/features/resources/agents/BuildStatus';
import { programNameIn } from '@/features/resources/programNames';
import type { HarnessBuild, RunningHarnessBuild } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { disabledFeatures, featureFallback, featureName } from './buildChecks';

export function DisabledBuildFeatures({ build, name }: { build: HarnessBuild; name?: string | undefined }) {
  const { t, i18n } = useTranslation();
  return <>{disabledFeatures(build).map((feature) => <div key={feature} className="text-sm wrap-anywhere text-text-muted" data-build-feature={feature}>
    <div>{t('programVersions.featureOff', { feature: featureName(feature, t), name: name ?? programNameIn(build, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown'), version: build.version })}</div>
    {featureFallback(build, feature, t) !== null && <div>{featureFallback(build, feature, t)}</div>}
  </div>)}</>;
}

export function ProgramBuild({ program, name }: { program: RunningHarnessBuild; name?: string | undefined }) {
  const { t, i18n } = useTranslation();
  const title = name ?? programNameIn(program, i18n.resolvedLanguage ?? i18n.language) ?? programNameIn(program.build, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown');
  return <div className="flex min-w-0 flex-col gap-1" data-program={program.kind}>
    <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm"><span>{title}</span><span className="min-w-0 wrap-anywhere font-mono">{program.version}</span>
      {program.build === null ? <span className="text-text-muted">{t('programVersions.unknownCheck')}</span> : <BuildStatus build={program.build} name={title} />}
    </div>
    {program.build !== null && <DisabledBuildFeatures build={program.build} name={title} />}
  </div>;
}

export function EnvironmentPrograms({ id, generation, compact = false }: { id: string; generation?: number; compact?: boolean }) {
  const { t } = useTranslation();
  const programs = useQuery(environmentProgramsQuery(id, generation));
  if (programs.isPending) return <SkeletonRows rows={2} />;
  if (programs.isError) return <ErrorState compact error={programs.error} onRetry={() => void programs.refetch()} />;
  return <section className="flex min-w-0 flex-col gap-2" aria-label={t('programVersions.running')}>
    {!compact && <h3 className="text-sm font-medium text-text-muted">{t('programVersions.running')}</h3>}
    {programs.data.length === 0 ? <StateLine compact message={t('programVersions.noneObserved')} /> : programs.data.map((program) => <ProgramBuild key={`${program.kind}:${program.version}`} program={program} />)}
  </section>;
}
