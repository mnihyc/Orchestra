/** What can be done with an environment, from its row in the list and from its own page: start or stop one that Orch made, enable or disable, open, edit, rename, copy its link and delete, each only where the reader may.
 *
 * The same acts serve the button and the menu at the end of a row, the menu under the right mouse button, the
 * acts rows that are ticked share, and the menu and the Edit button in the heading of the page. Starting and stopping
 * apply to an environment that Orch made; a directory on a person's machine has no unit to start. Disabling asks
 * first, naming the environment; deleting asks for its name, as the request does, and so is not an act that rows
 * share. An act that is under way shows on its row, and on the page, until the server has answered.
 */
import { resourceImpactQuery } from '@/api/resource-impact';
import { ImpactCounts } from '@/features/storage/Impact';
import { useResourceEnable, type EnableResource } from '@/features/storage/Enable';
import { SkeletonRows } from '@/ui/Skeleton';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { useResourceCleanup } from '@/features/storage/acts';
import { useMutation,useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { ArrowRightLeft, Ban, CirclePlay, Copy, ArrowRight, Pencil, Play, RefreshCw, RotateCcw, Square, TextCursorInput, Trash2, TriangleAlert } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { DeclareLostDialog } from '@/features/resources/DeclareLost';
import { openChange } from '@/features/change/target';
import { disableEnvironment, enableEnvironment, keepEnvironmentAwake, startEnvironment, stopEnvironment } from '@/api/environments';
import { type Act, type BulkAct, BulkNames, entriesOf, keysOf, type ListActs, type RowActs, useBusy, useCopyLink, shareAct } from '@/features/resources/acts';
import { EditEnvironment } from '@/features/resources/environments/Edit';
import type { EnvironmentTab } from '@/features/resources/environments/Environment';
import { RemoveEnvironmentDialog } from '@/features/resources/environments/Remove';
import { RestartEnvironmentDialog } from '@/features/resources/environments/Restart';
import { isBusy } from '@/features/resources/environments/words';
import type { Environment } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { adminSpec } from '@/ui/AdminAct';
import { toast } from '@/ui/Toast';
import { UpdateDialog } from '@/features/fleet/UpdateDialog';

export interface Asked {
  what: 'edit' | 'rename' | 'disable' | 'delete' | 'restart' | 'lost';
  environment: Environment;
}

export interface EnvironmentActs {
  list: ListActs<Environment>;
  /** Asks for an act that has a dialog, for a button of the page that is not in the menu. */
  ask: (what: Asked['what'], environment: Environment) => void;
  /** Reviews whether waiting messages run when the environment becomes available. */
  enable: (environment: Environment) => void;
  /** Keeps an environment awake: it counts as in use now. */
  keepAwake: (environment: Environment) => Act | null;
  page: (environment: Environment) => { edit: Act | null; primary: Act | null; menu: RowActs['entries'] };
  dialogs: ReactNode;
}

export function useEnvironmentActs(options: { onTab?: (environment: Environment, tab: EnvironmentTab) => void; afterDelete?: () => void } = {}): EnvironmentActs {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const copyLink = useCopyLink();
  const { busy, track } = useBusy();
  const cleanup = useResourceCleanup();
  const enablePlan = useResourceEnable();
  const enableResource = (environment:Environment):EnableResource => ({target:{object:{kind:'environment',id:environment.id},model_id:null},name:environment.name,admin:adminSpec(environment,'environment.manage'),enable:(runWaiting)=>enableEnvironment(environment.id,runWaiting)});
  const askEnable = (environment: Environment) => enablePlan.ask(enableResource(environment));
  const [asked, setAsked] = useState<Asked | null>(null);
  const [pauseDraft,setPauseDraft]=useState({id:'',text:''});
  const pauseNote=pauseDraft.id===asked?.environment.id?pauseDraft.text:'';
  const setPauseNote=(text:string)=>setPauseDraft({id:asked?.environment.id??'',text});
  const impact=useQuery({...resourceImpactQuery({object:{kind:'environment',id:asked?.environment.id??''},model_id:null}),enabled:asked?.what==='disable'||asked?.what==='delete'});
  const [updating, setUpdating] = useState<string[] | null>(null);

  const toTab = (environment: Environment, tab: EnvironmentTab) => {
    if (options.onTab !== undefined) options.onTab(environment, tab);
    else void navigate({ to: '/resources/environments/$id', params: { id: environment.id }, search: tab === 'overview' ? {} : { tab } });
  };
  const start = async (environment: Environment): Promise<unknown> => {
    const job = await startEnvironment(environment.id);
    return job;
  };
  const stop = async (environment: Environment): Promise<unknown> => {
    const stopped = await stopEnvironment(environment.id, false);
    return stopped;
  };
  const turnOff = async (environment: Environment): Promise<unknown> => {
    const updated = pauseNote.trim()===''?await disableEnvironment(environment.id):await disableEnvironment(environment.id,pauseNote.trim());
    return updated;
  };
  const disable = useMutation({
    mutationFn: turnOff,
    onSuccess: (_reply, environment) => {
      toast({ tone: 'success', title: t('environments.danger.disabled', { name: environment.name }) });
      setAsked(null);
    },
  });

  const canStart = (environment: Environment) => holds(environment.access, 'environment.manage') && environment.machine === null && !(environment.network.required_mode === 'restricted' && environment.network.mode === 'open') && (environment.status === 'stopped' || environment.status === 'none');
  const canStop = (environment: Environment) => holds(environment.access, 'environment.manage') && environment.machine === null && environment.status === 'running';

  const actsOf = (environment: Environment, where: 'row' | 'page'): RowActs & { edit: Act | null } => {
    const manage = holds(environment.access, 'environment.manage');
    const working = busy(environment.id) || isBusy(environment.status);
    const open: Act | null =
      where === 'row' ? { id: 'open', label: t('resources.act.open'), icon: <ArrowRight aria-hidden />, run: () => void navigate({ to: '/resources/environments/$id', params: { id: environment.id } }) } : null;
    const startAct: Act | null = canStart(environment)
      ? {
          id: 'start',
          label: t('resources.act.start'),
          icon: <Play aria-hidden />,
          pending: busy(environment.id),
          run: () =>
            track(environment.id, async () => {
              await start(environment);
              toast({ tone: 'success', title: t('environments.act.start') });
            }),
        }
      : null;
    const stopAct: Act | null = canStop(environment)
      ? {
          id: 'stop',
          label: t('resources.act.stop'),
          icon: <Square aria-hidden />,
          pending: busy(environment.id),
          run: () =>
            track(environment.id, async () => {
              await stop(environment);
              toast({ tone: 'success', title: t('environments.act.stop') });
            }),
        }
      : null;
    const enableAct: Act | null =
      manage && environment.status === 'disabled'
        ? {
            id: 'enable',
            label: t('resources.act.enable'),
            icon: <CirclePlay aria-hidden />,
            run: () => askEnable(environment),
          }
        : null;
    const disableAct: Act | null = manage && environment.status !== 'disabled' && environment.status !== 'lost' ? { id: 'disable', label: t('resources.act.disable'), icon: <Ban aria-hidden />, run: () => {setPauseNote('');setAsked({ what: 'disable', environment });} } : null;
    const edit: Act | null = manage ? { id: 'edit', label: t('resources.act.edit'), icon: <Pencil aria-hidden />, run: () => setAsked({ what: 'edit', environment }) } : null;
    const rename: Act | null = manage ? { id: 'rename', label: t('resources.act.rename'), icon: <TextCursorInput aria-hidden />, shortcut: 'F2', run: () => setAsked({ what: 'rename', environment }) } : null;
    const share = shareAct(t, { kind: 'environment', id: environment.id, name: environment.name }, environment.access);
    const copy: Act = { id: 'copy-link', label: t('resources.act.copyLink'), icon: <Copy aria-hidden />, run: () => copyLink(`/resources/environments/${environment.id}`) };
    const del: Act | null = manage ? { id: 'delete', label: t('trace.removeEntry'), icon: <Trash2 aria-hidden />, danger: true, run: () => setAsked({ what: 'delete', environment }) } : null;
    // What changes where and on what it runs, each a plan read before anything is done: for one that Orch made.
    const changed = manage && environment.machine === null && environment.status !== 'lost';
    const target = { kind: 'environment' as const, id: environment.id, name: environment.name };
    const updateAct: Act | null = changed ? { id: 'update', label: t('change.env.menu.update'), icon: <RefreshCw aria-hidden />, run: () => openChange(target, { kind: 'update' }) } : null;
    const recreateAct: Act | null = changed ? { id: 'recreate', label: t('change.env.menu.recreate'), icon: <RotateCcw aria-hidden />, run: () => openChange(target, { kind: 'recreate' }) } : null;
    const moveAct: Act | null = manage ? { id: 'move', label: t('change.env.menu.move'), icon: <ArrowRightLeft aria-hidden />, run: () => openChange(target, { kind: 'move' }) } : null;
    // A restart reviews the work that will stop before accepting its job.
    const restartAct:Act|null=canStop(environment) && environment.job?.state!=='queued' && environment.job?.state!=='running' && !(environment.network.required_mode==='restricted' && environment.network.mode==='open')
      ? {id:'restart',label:t('environmentRestart.action'),icon:<RotateCcw aria-hidden />,run:()=>setAsked({what:'restart',environment})}:null;
    const declare:Act|null = manage && environment.can_declare_lost ? { id: 'declare-lost', label: t('loss.declareMenu'), icon: <TriangleAlert aria-hidden />, danger: true, run: () => setAsked({ what: 'lost', environment }) } : null;
    const recover:Act|null = manage && environment.status === 'lost' ? { id: 'recover', label: t('loss.recover'), icon: <ArrowRightLeft aria-hidden />, run: () => openChange(target, { kind: 'recover' }) } : null;
    return {
      primary: startAct ?? stopAct ?? enableAct ?? open,
      edit,
      entries: entriesOf([open], [startAct, stopAct, restartAct, enableAct, disableAct], [edit, rename], [updateAct, recreateAct, environment.status === 'lost' ? null : moveAct, recover], [share, copy], [declare, del], [cleanup.act({kind:'environment',id:environment.id}, manage)]),
      keys: keysOf(rename),
      busy: where === 'row' && working,
    };
  };

  const bulk: BulkAct<Environment>[] = [
    { id: 'update', label: t('change.env.menu.update'), icon: <RefreshCw aria-hidden />, can: () => true,
      open: (rows) => setUpdating(rows.map((row) => row.id)), done: () => '' },
    { id: 'recover', label: t('loss.recover'), icon: <ArrowRightLeft aria-hidden />, can: (environment) => holds(environment.access, 'environment.manage'), open: (rows) => openChange({ kind: 'environments', ids: rows.map((row) => row.id) }, { kind: 'recover' }), done: () => '' },
    {
      id: 'start',
      label: t('resources.act.start'),
      icon: <Play aria-hidden />,
      can: canStart,
      run: start,
      done: (count) => t('environments.bulk.started', { count }),
    },
    {
      id: 'stop',
      label: t('resources.act.stop'),
      icon: <Square aria-hidden />,
      can: canStop,
      run: stop,
      done: (count) => t('environments.bulk.stopped', { count }),
    },
    {
      id: 'enable',
      label: t('resources.act.enable'),
      icon: <CirclePlay aria-hidden />,
      can: (environment) => holds(environment.access, 'environment.manage') && environment.status === 'disabled',
      open: (rows) => enablePlan.askMany(rows.map(enableResource)),
      done: (count) => t('environments.bulk.enabled', { count }),
    },
    {
      id: 'disable',
      label: t('resources.act.disable'),
      icon: <Ban aria-hidden />,
      can: (environment) => holds(environment.access, 'environment.manage') && environment.status !== 'disabled',
      run: turnOff,
      confirm: (rows) => ({
        title: t('environments.bulk.disableTitle', { count: rows.length }),
        body: <BulkNames names={rows.map((row) => row.name)} lead={t('environments.bulk.disableBody')} />,
        label: t('environments.bulk.disableConfirm', { count: rows.length }),
      }),
      done: (count) => t('environments.bulk.disabled', { count }),
    },
  ];

  const dialogs = (
    <>
      {cleanup.dialogs}
      {enablePlan.dialogs}
      {updating !== null && <UpdateDialog targets={{ environments: updating }} onClose={() => setUpdating(null)} />}
      {asked?.what === 'lost' && <DeclareLostDialog target={{ type: 'environment', id: asked.environment.id }} name={asked.environment.name} onClose={() => setAsked(null)} />}
      {asked !== null && (asked.what === 'edit' || asked.what === 'rename') && (
        <EditEnvironment
          key={`${asked.environment.id}-${asked.what}`}
          environment={asked.environment}
          {...(asked.what === 'rename' ? { only: 'name' as const } : {})}
          onTab={(tab) => toTab(asked.environment, tab)}
          onClose={() => setAsked(null)}
        />
      )}
      {asked?.what === 'delete' && <RemoveEnvironmentDialog environment={asked.environment} onClose={() => setAsked(null)} {...(options.afterDelete === undefined ? {} : { afterDelete: options.afterDelete })} />}
      {asked?.what === 'restart' && <RestartEnvironmentDialog environment={asked.environment} onClose={()=>setAsked(null)} />}
      <ConfirmDialog
        open={asked?.what === 'disable'}
        disabled={impact.isPending||impact.isError}
        admin={adminSpec(asked?.environment, 'environment.manage')}
        onOpenChange={(next) => !next && setAsked(null)}
        title={t('environments.danger.disableTitle', { name: asked?.environment.name ?? '' })}
        confirmLabel={t('environments.danger.disableConfirm', { name: asked?.environment.name ?? '' })}
        danger
        pending={disable.isPending}
        error={disable.error ?? undefined}
        onConfirm={() => asked !== null && disable.mutate(asked.environment)}
      >
        <div className="flex flex-col gap-3">{impact.isPending?<SkeletonRows rows={2} />:impact.isError?<ErrorState compact error={impact.error} onRetry={()=>void impact.refetch()} />:<ImpactCounts impact={impact.data} kind="disable" />}<Input label={t('resourceImpact.note')} optional value={pauseNote} onChange={(event)=>setPauseNote(event.target.value)} /></div>
      </ConfirmDialog>
    </>
  );

  return {
    list: { name: (environment) => environment.name, menuName: (environment) => environment.name, of: (environment) => actsOf(environment, 'row'), bulk, dialogs },
    ask: (what, environment) => setAsked({ what, environment }),
    enable: askEnable,
    keepAwake: (environment) =>
      holds(environment.access, 'environment.manage') && environment.machine === null && environment.status === 'running'
        ? {
            id: 'keep',
            label: t('environments.act.keepButton'),
            pending: busy(environment.id),
            run: () =>
              track(environment.id, async () => {
                await keepEnvironmentAwake(environment.id);
                toast({ tone: 'success', title: t('environments.act.keep') });
              }),
          }
        : null,
    page: (environment) => {
      const acts = actsOf(environment, 'page');
      return { edit: acts.edit, primary: acts.primary, menu: acts.entries };
    },
    dialogs,
  };
}
