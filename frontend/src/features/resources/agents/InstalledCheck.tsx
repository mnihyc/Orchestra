/** The own check of a program installed on a machine a person connected, run on that machine from the program's row. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { changeMachinePrograms } from '@/api/machinePrograms';
import { machineKey } from '@/api/resources';
import { StatusTag, type TagTone } from '@/features/admin/StatusTag';
import { PathValue } from '@/features/resources/TechnicalValue';
import type { InstalledProgram, InstalledProgramCheck, RunningHarnessBuild } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { RelativeTime } from '@/ui/RelativeTime';
import { Tooltip } from '@/ui/Tooltip';
import { featureName } from './buildChecks';

function tone(check: InstalledProgramCheck): TagTone {
  if (check.status === 'failed') return 'danger';
  if (check.status !== 'checked') return 'neutral';
  return check.off.length > 0 ? 'warning' : 'accent';
}

/** What the check found, in words: what the adapter lacks, what is switched off, or why it could not run. */
function Outcome({ check }: { check: InstalledProgramCheck }) {
  const { t } = useTranslation();
  const lines = [
    ...(check.missing.length > 0 && check.status === 'failed' ? [t('admin.harnesses.missing', { names: check.missing.join(', ') })] : []),
    ...check.off.map((feature) => t('programVersions.offInBuild', { feature: featureName(feature, t) })),
    ...(check.error !== null ? [check.error] : []),
  ];
  return lines.length === 0 ? null : <span className="wrap-anywhere text-sm text-text-muted">{lines.join(' · ')}</span>;
}

export function InstalledCheck({ program, installed, source, name }: { program: RunningHarnessBuild; installed: InstalledProgram; source: readonly unknown[]; name: string }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const run = useMutation({
    mutationFn: () => changeMachinePrograms(installed.machine_id, { action: 'check_harness', kind: program.kind, path: installed.path }),
    onSettled: () => {
      void cache.invalidateQueries({ queryKey: source, exact: true });
      void cache.invalidateQueries({ queryKey: machineKey(installed.machine_id), exact: true });
    },
  });
  const check = installed.check;
  const label = check === null ? t('admin.harnesses.status.untested') : check.status === 'untested' ? t('programVersions.couldNotCheck') : t(`admin.harnesses.status.${check.status}`);
  const tag = <StatusTag tone={check === null ? 'neutral' : tone(check)} className="min-w-max whitespace-nowrap">{label}</StatusTag>;
  return <span className="inline-flex min-w-0 flex-wrap items-baseline gap-x-1.5 gap-y-1" role="status" data-installed-check>
    {check === null ? tag : <Popover label={t('admin.harnesses.checkResults')} trigger={<Tooltip content={t('admin.harnesses.checkResults')}><button type="button" className="focus-inset min-w-0 max-w-full rounded-md text-left">{tag}</button></Tooltip>}>
      <div className="flex min-w-0 flex-col gap-2 text-sm wrap-anywhere">
        <span className="font-medium">{name} {check.version ?? program.version}</span>
        {check.builtin ? <span>{t('programVersions.builtIntoConnector')}</span> : <PathValue path={check.path} />}
        <Outcome check={check} />
        <span className="text-text-muted"><RelativeTime at={check.at} /></span>
      </div>
    </Popover>}
    {check?.builtin === true && <span className="text-sm text-text-muted">{t('programVersions.builtIntoConnector')}</span>}
    {check !== null && <Outcome check={check} />}
    {installed.can_check && <Button size="sm" variant="ghost" pending={run.isPending} onClick={() => run.mutate()}>{t('admin.harnesses.check')}</Button>}
    {run.isError && <ErrorAlert error={run.error} />}
  </span>;
}
