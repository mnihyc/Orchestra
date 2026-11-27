/** One line where a thread's tool access cannot be enforced where its agent runs, before anything is sent.
 *
 * The server compares the chosen tool access with what the agent program (or its executor) enforces there and
 * the widest the environment allows, as a turn is admitted. Where nothing at or below the choice can be
 * enforced, a turn would be refused: the line says so in the program's own words (where an executor carries out
 * the commands, in what that executor can enforce there) and offers the one choice that runs as the machine
 * allows. Nothing is changed by itself; the change is the person's, recorded as any change of tool access is.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { ShieldAlert } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { settingsKey, settingsQuery } from '@/api/settings';
import { setThreadSetting, threadKeys } from '@/api/threads';
import { sandboxCapability } from '@/features/resources/SandboxReadiness';
import type { ResolvedSetting, SandboxReadiness, Thread, ThreadParameters } from '@/generated/api-types';
import { offeredChoice } from '@/i18n/choices';
import { holds } from '@/lib/permissions';
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
    /** What stands in the way, in the words of the access control; nothing once the choice is written. */
    mutationFn: async (mode: Mode): Promise<string | null> => {
      // The revision already read by the access control is enough: a stale one is refused and said so.
      const settings = await queryClient.query({ ...settingsQuery(thread.id), staleTime: 'static' });
      const setting = settings.find((entry) => entry.key === 'sandbox.mode');
      const locked = setting?.locked_by;
      if (locked !== undefined && locked !== null) return t('composer.mode.locked', { place: locked.node?.name ?? t('composer.mode.placeAbove') });
      if (!setting?.writable) return t('composer.confinement.readOnly', { who: thread.owner.display_name });
      const confirmed = await setThreadSetting(thread.id, 'sandbox.mode', { value: mode, expected_revision: setting.own?.revision ?? 0 });
      queryClient.setQueryData<ResolvedSetting[]>(settingsKey(thread.id), (held) => held?.map((entry) => entry.key === confirmed.key
        ? { ...confirmed, available_values: confirmed.available_values ?? entry.available_values } : entry));
      // The line goes at once; the composer facts are read again in the background, and the thread's own line of
      // the change comes as every change's does.
      queryClient.setQueryData<ThreadParameters>(threadKeys.parameters(thread.id), (held) => held?.composer === null || held?.composer === undefined ? held
        : { ...held, composer: { ...held.composer, sandbox_mode: mode, sandbox_default: false, sandbox_shortfall: null } });
      void queryClient.invalidateQueries({ queryKey: threadKeys.parameters(thread.id) });
      return null;
    },
  });
  if (target === null || !manages) return null;
  const name = offeredChoice(t, 'sandboxLevel', target)?.name ?? target;
  const refused = choose.data ?? null;
  return <>
    <Button size="sm" variant="secondary" pending={choose.isPending} onClick={() => choose.mutate(target)} data-sandbox-run-as={target}>
      {t('composer.confinement.runAs', { mode: name })}
    </Button>
    {refused !== null && <span role="alert" className="text-xs text-danger-text">{refused}</span>}
    {choose.error !== null && <ErrorAlert error={choose.error} />}
  </>;
}

/** Why the line says it: the program's own sentence, or where an executor carries out the commands and says nothing of
 * itself, what that executor can enforce there, in the words the blocked thread's line and the environment's readiness
 * use. */
function shortfallReason(t: TFunction, report: SandboxReadiness, place: string | undefined): string | null {
  if (report.reason !== null) return report.reason;
  return report.executor === null || place === undefined ? null : sandboxCapability(report, place, t);
}

/** The line above the composer's controls: what cannot be enforced, the program's reason, and the way on. */
export function SandboxShortfallLine({ thread, report }: { thread: Thread; report: SandboxReadiness }) {
  const { t } = useTranslation();
  const mode = offeredChoice(t, 'sandboxLevel', report.requested)?.name ?? report.requested;
  const place = thread.environment?.name;
  const said = place === undefined ? t('composer.confinement.shortfallHere', { mode }) : t('composer.confinement.shortfall', { mode, place });
  const why = shortfallReason(t, report, place);
  return <div role="status" className="flex min-w-0 items-start gap-2 px-1 text-xs text-warning-text" data-sandbox-shortfall={report.requested}>
    <ShieldAlert className="mt-0.5 size-4 shrink-0" aria-hidden />
    <span className="min-w-0 flex-1 wrap-anywhere">{said}{why !== null && <span className="text-text-muted"> {why}</span>}
      {!managesAccess(thread) && runsAs(report) !== null && <span className="text-text-muted"> {t('composer.confinement.readOnly', { who: thread.owner.display_name })}</span>}</span>
    {managesAccess(thread) && <span className="flex shrink-0 flex-col items-end gap-1"><RunAsMachineAllows thread={thread} report={report} /></span>}
  </div>;
}
