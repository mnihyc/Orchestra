/** What runs on a host, from both pools and whoever's it is: each unit with the environment or provider it serves, where it stands and when it was last busy; one is stopped, leaving or interrupting what runs in it, or removed after naming it, with its volumes only when asked.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Boxes, Ellipsis, Square, Trash2 } from 'lucide-react';
import type { TFunction } from 'i18next';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { hostRuntimesQuery, removeHostRuntime, stopHostRuntime } from '@/api/resources';
import { Hint } from '@/features/help/Hint';
import type { Host, Runtime } from '@/generated/api-types';
import { DetailPages } from '@/features/resources/DetailSections';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot, type DotTone } from '@/ui/StatusDot';
import { type Column, Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { CopyButton } from '@/ui/CopyButton';
import { UpdateDialog, type UpdateTargets } from '@/features/fleet/UpdateDialog';

function runtimeName(runtime: Runtime, t: TFunction): string {
  return runtime.environment?.name ?? runtime.provider?.name ?? t('resourceDetails.unitGeneration', { generation: runtime.generation });
}

/** How a unit stands, as the colour of its dot: running is well, a failure is not, the rest is quiet. */
function stateTone(state: string): DotTone {
  if (state === 'running') return 'success';
  if (state === 'failed' || state === 'error') return 'danger';
  if (state === 'starting' || state === 'creating' || state === 'stopping') return 'warning';
  return 'idle';
}

const words = (state: string): string => state.replaceAll('_', ' ');

function RemoveRuntimeDialog({ host, runtime, onClose }: { host: Host; runtime: Runtime; onClose: () => void }) {
  const { t } = useTranslation();
  const name = runtimeName(runtime, t);
  const [typed, setTyped] = useState('');
  const [volumes, setVolumes] = useState(false);
  const remove = useMutation({
    mutationFn: () => removeHostRuntime(host.id, runtime.id, runtime.unit_name, volumes),
    onSuccess: () => {
      toast({ tone: 'success', title: t('hosts.runtimes.removed', { name }) });
      onClose();
    },
  });
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !remove.isPending && onClose()}
      title={t('hosts.runtimes.removeTitle', { name })}
      dismissible={!remove.isPending}
      footer={
        <>
          <Button disabled={remove.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="danger" pending={remove.isPending} blocked={typed !== name} onClick={() => remove.mutate()}>
            {t('hosts.runtimes.removeConfirm', { name })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 py-2">
        <p className="text-base text-text-muted">{t(runtime.kind === 'provider' ? 'hosts.runtimes.removeProviderBody' : 'hosts.runtimes.removeBody', { host: host.name })}</p>
        <p className="text-sm text-text-muted">{t('resourceDetails.unitGeneration', { generation: runtime.generation })}</p>
        <Checkbox label={t('hosts.runtimes.volumes')} help={<Hint id="host-volumes" />} checked={volumes} onCheckedChange={setVolumes} disabled={remove.isPending} />
        <CopyButton value={runtime.unit_name} label={t('resourceDetails.copyId')} />
        <Input label={t('hosts.runtimes.type', { name })} value={typed} onChange={(event) => setTyped(event.target.value)} autoComplete="off" disabled={remove.isPending} />
        {remove.error !== null && <ErrorAlert error={remove.error} />}
        {remove.error instanceof ApiError && remove.error.is('resource.in_use') && <p className="text-sm text-text-muted">{t('hosts.runtimes.inUse')}</p>}
      </div>
    </Dialog>
  );
}

export function HostRuntimes({ host }: { host: Host }) {
  const { t } = useTranslation();
  const list = useQuery(hostRuntimesQuery(host.id));
  const [interrupting, setInterrupting] = useState<Runtime | null>(null);
  const [removing, setRemoving] = useState<Runtime | null>(null);
  const [updating, setUpdating] = useState<UpdateTargets | null>(null);

  const stop = useMutation({
    mutationFn: ({ runtime, interrupt }: { runtime: Runtime; interrupt: boolean }) => stopHostRuntime(host.id, runtime.id, interrupt),
    onSuccess: (_stopped, { runtime }) => {
      toast({ tone: 'success', title: t('hosts.runtimes.stopped', { name: runtimeName(runtime, t) }) });
      setInterrupting(null);
    },
  });

  if (list.isPending) return <SkeletonRows rows={4} />;
  if (list.isError) return <ErrorState compact title={t('hosts.runtimes.title')} error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />;

  const columns: Column<Runtime>[] = [
    {
      id: 'unit',
      header: t('hosts.runtimes.unit'),
      cell: (runtime, shown) => {
        const withoutState = !shown.has('state');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <span className="flex min-w-0 flex-wrap items-center gap-2">
              {runtime.environment !== null ? (
                <Link to="/resources/environments/$id" params={{ id: runtime.environment.id }} className="truncate font-medium text-text hover:underline">
                  {runtime.environment.name}
                </Link>
              ) : runtime.provider !== null ? (
                <Link to="/resources/providers/$id" params={{ id: runtime.provider.id }} className="truncate font-medium text-text hover:underline">
                  {runtime.provider.name}
                </Link>
              ) : (
                <span className="truncate font-medium text-text">{runtimeName(runtime, t)}</span>
              )}
              <Badge>{t(`templates.kind.${runtime.kind}`)}</Badge>
              {(runtime.environment !== null || runtime.provider !== null) && <span className="text-xs text-text-muted">{t('resourceDetails.unitGeneration', { generation: runtime.generation })}</span>}
              <CopyButton value={runtime.unit_name} label={t('resourceDetails.copyId')} />
            </span>
            {withoutState && <StatusDot tone={stateTone(runtime.state)} label={words(runtime.state)} showLabel />}
          </div>
        );
      },
    },
    {
      id: 'state',
      header: t('hosts.runtimes.state'),
      from: 'sm',
      cell: (runtime) => (
        <span className="flex flex-col gap-1">
          <StatusDot tone={stateTone(runtime.state)} label={words(runtime.state)} showLabel />
          {runtime.state_detail !== null && <span className="text-sm text-text-muted">{runtime.state_detail}</span>}
          {runtime.successor_of !== null && <span className="text-sm text-text-muted">{t('hosts.runtimes.successor')}</span>}
          {runtime.remove_at !== null && (
            <span className="text-sm text-text-muted">
              {t('hosts.runtimes.removeAt')} <RelativeTime at={runtime.remove_at} />
            </span>
          )}
        </span>
      ),
    },
    { id: 'driver', header: t('hosts.runtimes.driver'), from: 'lg', className: 'text-text-muted', cell: (runtime) => runtime.driver },
    {
      id: 'machine',
      header: t('hosts.runtimes.machine'),
      from: 'lg',
      className: 'text-text-muted',
      cell: (runtime) => runtime.machine?.name ?? '–',
    },
    {
      id: 'active',
      header: t('hosts.runtimes.active'),
      from: 'md',
      className: 'text-text-muted',
      cell: (runtime) => (runtime.last_active_at === null ? t('hosts.runtimes.never') : <RelativeTime at={runtime.last_active_at} />),
    },
    {
      id: 'actions',
      header: t('hosts.runtimes.actions'),
      hideHeader: true,
      align: 'end',
      cell: (runtime) => (
        <Menu
          label={t('hosts.runtimes.menu', { name: runtimeName(runtime, t) })}
          align="end"
          entries={[
            { id: 'stop', label: t('hosts.runtimes.stop'), icon: <Square aria-hidden />, onSelect: () => stop.mutate({ runtime, interrupt: false }) },
            { id: 'interrupt', label: t('hosts.runtimes.stopInterrupt'), icon: <Square aria-hidden />, onSelect: () => setInterrupting(runtime) },
            { type: 'separator', id: 'apart' },
            { id: 'remove', label: t('hosts.runtimes.remove'), icon: <Trash2 aria-hidden />, danger: true, onSelect: () => setRemoving(runtime) },
          ]}
          trigger={<IconButton label={t('hosts.runtimes.menu', { name: runtimeName(runtime, t) })} icon={<Ellipsis aria-hidden />} />}
        />
      ),
    },
  ];

  return (
    <Section id="host-runtimes" title={t('hosts.runtimes.title')} lead={t('hosts.runtimes.lead')}>
      {list.data.length === 0 ? (
        <EmptyState compact icon={<Boxes aria-hidden />} title={t('hosts.runtimes.none')} description={t('hosts.runtimes.noneLead')} />
      ) : (
        <>
          <Button className="self-start" size="sm" onClick={() => setUpdating({ environments: list.data.flatMap((runtime) => runtime.environment === null ? [] : [runtime.environment.id]), providers: list.data.flatMap((runtime) => runtime.provider === null ? [] : [runtime.provider.id]) })}>{t('change.env.menu.update')}</Button>
          <DetailPages items={list.data}>{(page) => <Table label={t('hosts.runtimes.title')} columns={columns} rows={page} rowKey={(runtime) => runtime.id} />}</DetailPages>
        </>
      )}
      {updating !== null && <UpdateDialog targets={updating} onClose={() => setUpdating(null)} />}
      {stop.error !== null && interrupting === null && <ErrorAlert error={stop.error} />}
      <ConfirmDialog
        open={interrupting !== null}
        onOpenChange={(next) => !next && setInterrupting(null)}
        title={t('hosts.runtimes.interruptTitle', { name: interrupting === null ? '' : runtimeName(interrupting, t) })}
        confirmLabel={t('hosts.runtimes.interruptConfirm', { name: interrupting === null ? '' : runtimeName(interrupting, t) })}
        danger
        pending={stop.isPending}
        error={stop.error ?? undefined}
        onConfirm={() => interrupting !== null && stop.mutate({ runtime: interrupting, interrupt: true })}
      >
        {t('hosts.runtimes.interruptBody')}
      </ConfirmDialog>
      {removing !== null && <RemoveRuntimeDialog host={host} runtime={removing} onClose={() => setRemoving(null)} />}
    </Section>
  );
}
