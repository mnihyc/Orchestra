import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { changeMachinePrograms } from '@/api/machinePrograms';
import { machineKey, machineQuery } from '@/api/resources';
import { PathValue } from '@/features/resources/TechnicalValue';
import { readMachineFacts } from '@/features/resources/machines/facts';
import type { RunningHarnessBuild } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';

function reader() {
  const { userId, readerVersion, status } = client.session.getState();
  return JSON.stringify([userId, readerVersion, status]);
}

function Identity({ machineId, program, source }: Props) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const machine = useQuery(machineQuery(machineId));
  const refresh = useMutation({
    mutationFn: async () => ({ owner: reader(), result: await changeMachinePrograms(machineId, { action: 'reload' }) }),
    onSuccess: ({ owner, result }) => {
      if (reader() !== owner) return;
      if (result.machine_view?.id === machineId) cache.setQueryData(machineKey(machineId), result.machine_view);
      else void cache.invalidateQueries({ queryKey: machineKey(machineId), exact: true });
      void cache.invalidateQueries({ queryKey: source, exact: true });
    },
  });
  if (machine.isPending) return <SkeletonRows rows={2} read={machine} />;
  if (machine.isError) return <ErrorState compact error={machine.error} onRetry={() => void machine.refetch()} />;
  const facts = readMachineFacts(machine.data.facts);
  const policy = facts.policy?.['agent.harnesses'];
  const selected: unknown = Array.isArray(policy) ? policy.find((row: unknown) =>
    typeof row === 'object' && row !== null && 'kind' in row && row.kind === program.kind) : null;
  const path = selected && typeof selected === 'object' && 'path' in selected ? selected.path : null;
  const candidates = facts.harnesses.filter((entry) => entry.kind === program.kind && entry.version === program.version &&
    typeof path === 'string' && (entry.path === path || entry.path.split(/[/\\]/u).at(-1) === path));
  const observed = candidates.length === 1 ? candidates[0] : undefined;
  const builtin = facts.adapters.some((adapter) => adapter.kind === program.kind && adapter.builtin && adapter.needsBinary === false);
  return <div className="flex min-w-0 flex-col items-start gap-2 text-sm">
    <span className="font-medium">{machine.data.name}</span>
    {builtin && <span>{t('programVersions.builtIntoConnector')}</span>}
    {observed && <PathValue path={observed.path} />}
    {observed?.sha256 && <div>SHA-256 <code className="break-all">{observed.sha256}</code></div>}
    {!builtin && <span>{t('composer.approvalUnavailable.selectBuild')}</span>}
    {!builtin && !observed?.sha256 && <span>{t('programVersions.noFingerprint')}</span>}
    {holds(machine.data.access, 'machine.manage') && <Button size="sm" pending={refresh.isPending} onClick={() => refresh.mutate()}>{t('machinePrograms.nativeRefresh')}</Button>}
    {refresh.isError && <ErrorAlert error={refresh.error} />}
    {refresh.isSuccess && refresh.data.owner === reader() && <span role="status">{t('programVersions.identityRefreshed')}</span>}
  </div>;
}

interface Props {
  machineId: string;
  program: RunningHarnessBuild;
  source: readonly unknown[];
}

/** Read the selected machine's authorized native report only when a person asks to identify a program. */
export function IdentifyProgram(props: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return <Popover open={open} onOpenChange={setOpen} label={t('programVersions.identify')}
    trigger={<Button size="sm" variant="ghost">{t('programVersions.identify')}</Button>}>
    {open && <Identity {...props} />}
  </Popover>;
}
