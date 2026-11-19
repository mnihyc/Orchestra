/** One explicit, audited page grant before an administrator manages a person's machine. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { changePage, currentPageOf, pageKey, pageQuery, previewPage } from '@/api/access';
import { meQuery } from '@/api/auth';
import { machineKey, machinesKey } from '@/api/resources';
import type { Machine, PageChange } from '@/generated/api-types';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

/** The server's ordinary machine or host management grant. */
export function holdsMachineManagement(machine: Machine): boolean {
  return machine.access?.permissions.some((permission) => permission === 'machine.manage' || permission === 'host.manage') === true;
}

/** Administration can review a page grant, while a host keeps its normal Host Manage path. */
export function needsMachineManagementOverride(machine: Machine): boolean {
  return !machine.roles.includes('node') && machine.access?.administers === true && !holdsMachineManagement(machine);
}

export function useMachineManagement() {
  const [asked, setAsked] = useState<{ machine: Machine; act: () => void } | null>(null);
  return {
    run: (machine: Machine, act: () => void) => {
      if (needsMachineManagementOverride(machine)) setAsked({ machine, act });
      else act();
    },
    dialog: asked === null ? null : <ManagementOverride machine={asked.machine} onClose={() => setAsked(null)} onGranted={() => {
      setAsked(null);
      asked.act();
    }} />,
  };
}

function ManagementOverride({ machine, onClose, onGranted }: { machine: Machine; onClose: () => void; onGranted: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const target = { kind: 'machine' as const, id: machine.id };
  const me = useQuery(meQuery);
  const page = useQuery(pageQuery(target));
  const change = useMemo<PageChange | null>(() => me.data === undefined || page.data === undefined ? null : ({
    expected_revision: page.data.revision,
    rows: [{ permission: 'machine.manage', add: [{ holder: { kind: 'user', id: me.data.id } }] }],
  }), [me.data, page.data]);
  const preview = useQuery({
    queryKey: ['machine-management-override', machine.id, change],
    queryFn: () => change === null ? Promise.reject(new Error(t('machineManagement.notReady'))) : previewPage(target, change),
    enabled: change !== null,
  });
  const grant = useMutation({
    mutationFn: () => change === null ? Promise.reject(new Error(t('machineManagement.notReady'))) : changePage(target, change),
    onError: (error) => {
      const current = currentPageOf(error);
      if (current !== null) client.setQueryData(pageKey(target), current);
    },
    onSuccess: async () => {
      await Promise.all([
        client.invalidateQueries({ queryKey: pageKey(target) }),
        client.invalidateQueries({ queryKey: machineKey(machine.id) }),
        client.invalidateQueries({ queryKey: machinesKey }),
      ]);
      onGranted();
    },
  });
  const ready = change !== null && preview.isSuccess && !preview.isFetching && !me.isError && !page.isError;
  const error = grant.error ?? preview.error ?? page.error ?? me.error;
  const review = () => { grant.reset(); void me.refetch(); void page.refetch(); void preview.refetch(); };
  return <ConfirmDialog open onOpenChange={(open) => !open && onClose()}
    title={<span className="break-words">{t('machineManagement.overrideTitle', { name: machine.name })}</span>}
    confirmLabel={t('machineManagement.continue')} pending={grant.isPending} disabled={!ready}
    override={{ hint: t('machineManagement.audit'), additions: [<span className="inline-flex min-w-0 flex-wrap items-center gap-2">{me.data === undefined ? <SkeletonRows rows={1} /> : <UserChip user={me.data} />}{t('machineManagement.adds')}</span>] }}
    onConfirm={() => grant.mutate()}>
    <span className="break-words">{t('machineManagement.effect', { name: machine.name })}</span>
    {error !== null && <ErrorAlert error={error} action={<Button size="sm" onClick={review}>{t('common.tryAgain')}</Button>} />}
  </ConfirmDialog>;
}
