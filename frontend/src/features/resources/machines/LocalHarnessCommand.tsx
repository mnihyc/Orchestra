/** The exact local command that accepts or withdraws one agent program on a machine that keeps its agent programs in its own
 * policy, and Refresh, which reads the machine again once the command has run there. The guided installed sign-in and each row of
 * a machine's Programs tab use this one composition. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { defaultProviderReader } from '@/api/defaultsOpening';
import { machineKey, probeMachine } from '@/api/resources';
import type { Machine } from '@/generated/api-types';
import { CommandBox } from '@/features/resources/Facts';
import { harnessPolicyWith, readMachineFacts } from '@/features/resources/machines/facts';
import { installationOf } from '@/features/resources/machines/Installation';
import { policyCommand } from '@/features/resources/machines/Policy';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

type LocalMachine = Pick<Machine, 'id' | 'facts'> & Partial<Pick<Machine, 'installation' | 'platform_os'>>;

/** The command for this machine, or null while it has not reported its program list or its policy command. */
export function localHarnessCommand(machine: LocalMachine, program: { kind: string; path: string }, accept: boolean): string | null {
  const current = readMachineFacts(machine.facts).policy?.['agent.harnesses'];
  const policy = (machine.installation ?? installationOf(machine.facts))?.commands.policy;
  if (!Array.isArray(current) || policy === undefined) return null;
  return policyCommand('agent.harnesses', harnessPolicyWith(current, program, accept), policy, machine.platform_os === 'windows');
}

/** Reads the machine again and keeps its answer, unless the page's reader changed meanwhile. */
export function useMachineRefresh(machineId: string) {
  const cache = useQueryClient();
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  return useMutation({
    mutationFn: async () => { const reader = defaultProviderReader(); return { reader, machine: await probeMachine(machineId) }; },
    onSuccess: (answer) => { if (mounted.current && defaultProviderReader() === answer.reader) cache.setQueryData(machineKey(machineId), answer.machine); },
  });
}

export function LocalHarnessCommand({ machine, program, accept }: { machine: LocalMachine; program: { kind: string; path: string }; accept: boolean }) {
  const { t } = useTranslation();
  const refresh = useMachineRefresh(machine.id);
  const command = localHarnessCommand(machine, program, accept);
  return <div className="flex min-w-0 flex-col items-start gap-2">
    {command !== null ? <CommandBox command={command} label={t('machines.policy.copy')} />
      : <span className="text-sm text-text-muted">{t('machinePrograms.nativeCommandNotReported')}</span>}
    <Button size="sm" pending={refresh.isPending} onClick={() => refresh.mutate()}>{t('machinePrograms.nativeRefresh')}</Button>
    {refresh.error !== null && <ErrorAlert error={refresh.error} />}
  </div>;
}
