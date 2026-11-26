/** One line where a thread's tool access cannot be enforced where its agent runs, before anything is sent.
 *
 * The server compares the chosen tool access with what the agent program (or its executor) enforces there and
 * the widest the environment allows, as a turn is admitted. Where nothing at or below the choice can be
 * enforced, a turn would be refused: the line says so in the program's own words and offers the one choice
 * that runs as the machine allows. Nothing is changed by itself; the change is the person's, recorded as any
 * change of tool access is.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { ShieldAlert } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { settingsKey, settingsQuery } from '@/api/settings';
import { setThreadSetting, threadKeys } from '@/api/threads';
import type { SandboxReadiness, Thread } from '@/generated/api-types';
import { offeredChoice } from '@/i18n/choices';
import { holds } from '@/lib/permissions';
import { refreshThread } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

type Mode = SandboxReadiness['requested'];
const ORDER: Record<Mode, number> = { read_only: 0, workspace: 1, open: 2 };

/** The widest mode the machine enforces for this agent within what the environment allows; none where nothing fits. */
export function runsAs(report: SandboxReadiness): Mode | null {
  return report.enforceable
    .filter((mode) => report.agent_modes.length === 0 || report.agent_modes.includes(mode))
    .filter((mode) => ORDER[mode] <= ORDER[report.widest])
    .reduce<Mode | null>((widest, mode) => (widest === null || ORDER[mode] > ORDER[widest] ? mode : widest), null);
}

function managesAccess(thread: Thread): boolean {
  return !thread.archived && thread.side_of === null && holds(thread.access, 'thread.manage');
}

/** The act that runs a thread as its machine allows: its own tool access becomes that mode, as a person's choice. */
export function RunAsMachineAllows({ thread, report }: { thread: Thread; report: SandboxReadiness }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const target = runsAs(report);
  const manages = managesAccess(thread);
  const choose = useMutation({
    mutationFn: async (mode: Mode) => {
      const settings = await queryClient.fetchQuery({ ...settingsQuery(thread.id), staleTime: 0 });
      const setting = settings.find((entry) => entry.key === 'sandbox.mode');
      const locked = setting?.locked_by;
      if (locked !== undefined && locked !== null) throw new Error(t('composer.mode.locked', { place: locked.node?.name ?? t('composer.mode.placeAbove') }));
      if (setting === undefined || !setting.writable) throw new Error(t('composer.confinement.readOnly', { who: thread.owner.display_name }));
      await setThreadSetting(thread.id, 'sandbox.mode', { value: mode, expected_revision: setting.own?.revision ?? 0 });
    },
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: threadKeys.parameters(thread.id) }),
        queryClient.invalidateQueries({ queryKey: settingsKey(thread.id) }),
      ]);
      refreshThread(thread.id);
    },
  });
  if (target === null || !manages) return null;
  const name = offeredChoice(t, 'sandboxLevel', target)?.name ?? target;
  return <>
    <Button size="sm" variant="secondary" pending={choose.isPending} onClick={() => choose.mutate(target)} data-sandbox-run-as={target}>
      {t('composer.confinement.runAs', { mode: name })}
    </Button>
    {choose.error !== null && <ErrorAlert error={choose.error} />}
  </>;
}

/** The line above the composer's controls: what cannot be enforced, the program's reason, and the way on. */
export function SandboxShortfallLine({ thread, report }: { thread: Thread; report: SandboxReadiness }) {
  const { t } = useTranslation();
  const mode = offeredChoice(t, 'sandboxLevel', report.requested)?.name ?? report.requested;
  const place = thread.environment?.name;
  const said = place === undefined ? t('composer.confinement.shortfallHere', { mode }) : t('composer.confinement.shortfall', { mode, place });
  return <div role="status" className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-warning-text" data-sandbox-shortfall={report.requested}>
    <ShieldAlert className="size-4 shrink-0" aria-hidden />
    <span className="min-w-0 wrap-anywhere">{said}{report.reason !== null && <span className="text-text-muted"> {report.reason}</span>}</span>
    {managesAccess(thread) ? <RunAsMachineAllows thread={thread} report={report} />
      : runsAs(report) !== null && <span className="text-text-muted">{t('composer.confinement.readOnly', { who: thread.owner.display_name })}</span>}
  </div>;
}
