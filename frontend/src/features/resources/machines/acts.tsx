/** What can be done with a machine, from its row in the list and from its own page: enroll it again, refresh what it reports, open, edit, rename, copy its link and revoke it.
 *
 * A machine belongs to the person who enrolled it, who is the only one who sees it here, so every act is theirs
 * and none is hidden; one that cannot be done now stays in the menu with the reason (a machine that is away
 * cannot be asked what it reports, and one that was revoked is revoked). A machine that lost its enrollment
 * (revoked, or its certificate has ended) offers Enroll again as the act most wanted of it. The same acts serve
 * the button and the menu at the end of a row, the menu under the right mouse button, the acts rows that are
 * ticked share, and the menu and the Edit button in the heading of the page.
 */
import { useResourceCleanup } from '@/features/storage/acts';
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { Copy, ArrowRight, KeyRound, Pencil, RefreshCw, ShieldOff, TriangleAlert, ArrowRightLeft, TextCursorInput } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { meQuery } from '@/api/auth';
import { DeclareLostDialog } from '@/features/resources/DeclareLost';
import { openChange } from '@/features/change/target';
import { probeMachine, revokeMachine } from '@/api/resources';
import { useComponentUpdateAct } from '@/features/resources/ComponentUpdate';
import { type Act, type BulkAct, BulkNames, entriesOf, keysOf, type ListActs, type RowActs, useBusy, useCopyLink } from '@/features/resources/acts';
import { EditMachine } from '@/features/resources/machines/Edit';
import { EnrollAgainDialog, lostEnrollment } from '@/features/resources/machines/EnrollAgain';
import { holdsMachineManagement, needsMachineManagementOverride, useMachineManagement } from '@/features/resources/machines/Management';
import type { Machine } from '@/generated/api-types';
import { useNow } from '@/lib/time';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { adminSpec } from '@/ui/AdminAct';
import { toast } from '@/ui/Toast';

export interface Asked {
  what: 'edit' | 'rename' | 'revoke' | 'again' | 'lost';
  machine: Machine;
}

export interface MachineActs {
  list: ListActs<Machine>;
  /** Asks for an act that has a dialog, for a button of the page that is not in the menu. */
  ask: (what: Asked['what'], machine: Machine) => void;
  page: (machine: Machine) => { edit: Act | null; primary: Act | null; menu: RowActs['entries'] };
  dialogs: ReactNode;
}

export function useMachineActs(): MachineActs {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const now = useNow();
  const me = useQuery(meQuery);
  const copyLink = useCopyLink();
  const { busy, track } = useBusy();
  const cleanup = useResourceCleanup();
  const [asked, setAsked] = useState<Asked | null>(null);
  const componentUpdate = useComponentUpdateAct();
  const management = useMachineManagement();
  const ask = (what: Asked['what'], machine: Machine) => management.run(machine, () => setAsked({ what, machine }));

  const refresh = (machine: Machine): Promise<Machine> => probeMachine(machine.id);
  const revokeOne = (machine: Machine): Promise<Machine> => revokeMachine(machine.id);
  const probe = useMutation({
    mutationFn: refresh,
    onSuccess: (machine) => toast({ tone: 'success', title: t('machines.probe.done', { name: machine.name }) }),
    onError: () => toast({ tone: 'danger', title: t('machines.probe.failed') }),
  });
  const revoke = useMutation({
    mutationFn: revokeOne,
    onSuccess: (machine) => {
      toast({ tone: 'success', title: t('machines.revoke.done', { name: machine.name }) });
      setAsked(null);
    },
  });

  const actsOf = (machine: Machine, where: 'row' | 'page'): RowActs & { edit: Act | null } => {
    const manages = holdsMachineManagement(machine) || needsMachineManagementOverride(machine);
    const active = machine.status === 'active';
    const update = componentUpdate({kind:'machine',id:machine.id,update:machine.software_update,manage:manages && active,online:machine.online,admin:adminSpec(machine,['machine.manage','host.manage'])});
    const updateNow = update === null ? null : { ...update, run: () => management.run(machine, update.run) };
    const lost = lostEnrollment(machine, now);
    const open: Act | null =
      where === 'row' ? { id: 'open', label: t('resources.act.open'), icon: <ArrowRight aria-hidden />, run: () => void navigate({ to: '/resources/machines/$id', params: { id: machine.id } }) } : null;
    const again: Act = { id: 'again', label: t('resources.act.enrollAgain'), icon: <KeyRound aria-hidden />, run: () => ask('again', machine) };
    const reload: Act = {
      id: 'refresh',
      label: t('resources.act.refresh'),
      icon: <RefreshCw aria-hidden />,
      disabled: !active ? t('machines.probe.revoked') : !machine.online ? t('machines.probe.offline') : false,
      pending: probe.isPending && probe.variables.id === machine.id,
      run: () => management.run(machine, () =>
        where === 'row'
          ? track(machine.id, async () => {
              const updated = await refresh(machine);
              toast({ tone: 'success', title: t('machines.probe.done', { name: updated.name }) });
            })
          : probe.mutate(machine)),
    };
    const edit: Act = { id: 'edit', label: t('resources.act.edit'), icon: <Pencil aria-hidden />, run: () => ask('edit', machine) };
    const rename: Act = { id: 'rename', label: t('resources.act.rename'), icon: <TextCursorInput aria-hidden />, shortcut: 'F2', run: () => ask('rename', machine) };
    const copy: Act = { id: 'copy-link', label: t('resources.act.copyLink'), icon: <Copy aria-hidden />, run: () => copyLink(`/resources/machines/${machine.id}`) };
    const revokeAct: Act = { id: 'revoke', label: t('resources.act.revoke'), icon: <ShieldOff aria-hidden />, danger: true, disabled: active ? false : t('machines.revoke.already'), run: () => ask('revoke', machine) };
    const declare: Act | null = !machine.online && machine.lost_at === undefined && (machine.owner?.id === me.data?.id || machine.access?.admin_only?.includes('machine.manage')===true) ? { id: 'declare-lost', label: t('loss.declareMenu'), icon: <TriangleAlert aria-hidden />, danger: true, run: () => setAsked({ what: 'lost', machine }) } : null;
    const recover: Act | null = machine.lost_at !== undefined && (machine.owner?.id === me.data?.id || machine.access?.admin_only?.includes('machine.manage')===true) ? { id: 'recover', label: t('loss.recover'), icon: <ArrowRightLeft aria-hidden />, run: () => openChange({ kind: 'machine', id: machine.id }, { kind: 'recover' }) } : null;
    return {
      // What a machine that lost its enrollment needs is a way back; one that is well, to be asked what it reports; one that is away, to be opened.
      primary: updateNow ?? (manages ? machine.lost_at !== undefined ? recover : lost !== null ? again : active && machine.online ? reload : open : open),
      edit: manages ? edit : null,
      entries: manages ? entriesOf([open], [updateNow, machine.lost_at === undefined ? again : null, reload, recover], [edit, rename], [copy], [declare, revokeAct], [cleanup.act({kind:'machine',id:machine.id},manages)]) : entriesOf([open],[copy],[cleanup.act({kind:'machine',id:machine.id},false)]),
      keys: manages ? keysOf(rename) : {},
      busy: busy(machine.id),
    };
  };

  const bulk: BulkAct<Machine>[] = [
    {
      id: 'refresh',
      label: t('resources.act.refresh'),
      icon: <RefreshCw aria-hidden />,
      can: (machine) => holdsMachineManagement(machine) && machine.status === 'active' && machine.online,
      run: refresh,
      done: (count) => t('machines.bulk.refreshed', { count }),
    },
    {
      id: 'revoke',
      label: t('resources.act.revoke'),
      icon: <ShieldOff aria-hidden />,
      danger: true,
      can: (machine) => holdsMachineManagement(machine) && machine.status === 'active',
      run: revokeOne,
      confirm: (rows) => ({
        title: t('machines.bulk.revokeTitle', { count: rows.length }),
        body: <BulkNames names={rows.map((row) => row.name)} lead={t('machines.revoke.body')} />,
        label: t('machines.bulk.revokeConfirm', { count: rows.length }),
      }),
      done: (count) => t('machines.bulk.revoked', { count }),
    },
  ];

  const dialogs = (
    <>
      {cleanup.dialogs}
      {asked?.what === 'lost' && <DeclareLostDialog target={{ type: 'machine', id: asked.machine.id }} name={asked.machine.name} onClose={() => setAsked(null)} />}
      {management.dialog}
      {asked !== null && (asked.what === 'edit' || asked.what === 'rename') && (
        <EditMachine key={`${asked.machine.id}-${asked.what}`} machine={asked.machine} {...(asked.what === 'rename' ? { only: 'name' as const } : {})} onClose={() => setAsked(null)} />
      )}
      {asked?.what === 'again' && <EnrollAgainDialog key={asked.machine.id} machine={asked.machine} onClose={() => setAsked(null)} />}
      <ConfirmDialog
        open={asked?.what === 'revoke'}
        admin={adminSpec(asked?.machine, ['machine.manage', 'host.manage'])}
        onOpenChange={(next) => !next && setAsked(null)}
        title={t('machines.revoke.title', { name: asked?.machine.name ?? '' })}
        confirmLabel={t('machines.revoke.confirm', { name: asked?.machine.name ?? '' })}
        danger
        pending={revoke.isPending}
        error={revoke.error ?? undefined}
        onConfirm={() => asked !== null && revoke.mutate(asked.machine)}
      >
        {t('machines.revoke.body')}
      </ConfirmDialog>
    </>
  );

  return {
    list: { name: (machine) => machine.name, menuName: (machine) => machine.name, of: (machine) => actsOf(machine, 'row'), bulk, dialogs },
    ask,
    page: (machine) => {
      const acts = actsOf(machine, 'page');
      return { edit: acts.edit, primary: acts.primary, menu: acts.entries };
    },
    dialogs,
  };
}
