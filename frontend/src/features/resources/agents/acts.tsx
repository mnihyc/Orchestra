/** What can be done with an agent profile, from its row in the list and from its own page: open, switch on or off, edit, rename, copy its link and delete, each only where the reader may.
 *
 * The same acts serve the button and the menu at the end of a row, the menu under the right mouse button, the
 * acts rows that are ticked share, and the menu and the Edit button in the heading of the page. Switching
 * off and deleting ask first, naming the profile. Running turns stop immediately while bound
 * threads retain their conversations and waiting inputs.
 */
import { resourceImpactQuery } from '@/api/resource-impact';
import { ImpactCounts } from '@/features/storage/Impact';
import { useResourceEnable, type EnableResource } from '@/features/storage/Enable';
import { SkeletonRows } from '@/ui/Skeleton';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { useMutation,useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { Ban, CirclePlay, Copy, ArrowRight, ArrowRightLeft, Pencil, TextCursorInput, Trash2 } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { openChange } from '@/features/change/target';
import { deleteAgent, updateAgent } from '@/api/resources';
import { type Act, type BulkAct, BulkNames, entriesOf, keysOf, type ListActs, type RowActs, useBusy, useCopyLink, shareAct } from '@/features/resources/acts';
import { EditAgent } from '@/features/resources/agents/Edit';
import type { Agent } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { toast } from '@/ui/Toast';

interface Asked {
  what: 'edit' | 'rename' | 'disable' | 'delete';
  agent: Agent;
}

export interface AgentActs {
  /** What the list gives its rows. */
  list: ListActs<Agent>;
  /** What the page of one gives its heading: the Edit act, the act most used and the menu. */
  page: (agent: Agent) => { edit: Act | null; primary: Act | null; menu: RowActs['entries'] };
  /** The dialogs the acts open, for whichever draws them. */
  dialogs: ReactNode;
}

/** The page of a profile says how its Profile tab is opened, and what follows a delete. */
export function useAgentActs(options: { onProfile?: (agent: Agent) => void; afterDelete?: () => void } = {}): AgentActs {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const copyLink = useCopyLink();
  const { busy } = useBusy();
  const enablePlan = useResourceEnable();
  const enableResource = (agent:Agent):EnableResource => ({target:{object:{kind:'agent',id:agent.id},model_id:null},name:agent.name,admin:adminSpec(agent,'agent.manage'),enable:(runWaiting)=>updateAgent(agent.id,{expected_revision:agent.revision,disabled:false},undefined,runWaiting)});
  const [asked, setAsked] = useState<Asked | null>(null);
  const [typed,setTyped]=useState('');
  const [pauseDraft,setPauseDraft]=useState({id:'',text:''});
  const pauseNote=pauseDraft.id===asked?.agent.id?pauseDraft.text:'';
  const setPauseNote=(text:string)=>setPauseDraft({id:asked?.agent.id??'',text});
  const impact=useQuery({...resourceImpactQuery({object:{kind:'agent',id:asked?.agent.id??''},model_id:null}),enabled:asked?.what==='disable'||asked?.what==='delete'});

  /** Switches a profile on or off, on the revision that was read, and tells what keeps the profile. */
  const setDisabled = (agent: Agent, disabled: boolean): Promise<Agent> =>
    disabled&&pauseNote.trim()!==''?updateAgent(agent.id, { expected_revision: agent.revision, disabled },pauseNote.trim()):updateAgent(agent.id, { expected_revision: agent.revision, disabled });
  const dropAgent = (agent: Agent): Promise<void> => deleteAgent(agent.id);

  const toggle = useMutation({
    mutationFn: ({ agent, disabled }: { agent: Agent; disabled: boolean }) => setDisabled(agent, disabled),
    onSuccess: (updated) => {
      toast({ tone: 'success', title: updated.disabled ? t('agents.danger.disabled', { name: updated.name }) : t('agents.danger.enabled', { name: updated.name }) });
      setAsked(null);
    },
  });
  const remove = useMutation({
    mutationFn: dropAgent,
    onSuccess: (_reply, agent) => {
      toast({ tone: 'success', title: t('agents.danger.deleted', { name: agent.name }) });
      setAsked(null);
      options.afterDelete?.();
    },
  });

  const actsOf = (agent: Agent, where: 'row' | 'page'): RowActs & { edit: Act | null } => {
    const manage = holds(agent.access, 'agent.manage');
    const open: Act | null =
      where === 'row' ? { id: 'open', label: t('resources.act.open'), icon: <ArrowRight aria-hidden />, run: () => void navigate({ to: '/resources/agents/$id', params: { id: agent.id } }) } : null;
    const enable: Act | null = manage && agent.disabled ? { id: 'enable', label: t('resources.act.enable'), icon: <CirclePlay aria-hidden />, run: () => enablePlan.ask(enableResource(agent)) } : null;
    const disable: Act | null = manage && !agent.disabled ? { id: 'disable', label: t('resources.act.disable'), icon: <Ban aria-hidden />, run: () => {setPauseNote('');setAsked({ what: 'disable', agent });} } : null;
    const edit: Act | null = manage ? { id: 'edit', label: t('resources.act.edit'), icon: <Pencil aria-hidden />, run: () => setAsked({ what: 'edit', agent }) } : null;
    const rename: Act | null = manage ? { id: 'rename', label: t('resources.act.rename'), icon: <TextCursorInput aria-hidden />, shortcut: 'F2', run: () => setAsked({ what: 'rename', agent }) } : null;
    const share = shareAct(t, { kind: 'agent', id: agent.id, name: agent.name }, agent.access);
    const switchThreads:Act={id:'switch-threads',label:t('resources.act.switchThreads'),icon:<ArrowRightLeft aria-hidden />,run:()=>openChange({kind:'agent',id:agent.id,name:agent.name},{kind:'switch'})};
    const copy: Act = { id: 'copy-link', label: t('resources.act.copyLink'), icon: <Copy aria-hidden />, run: () => copyLink(`/resources/agents/${agent.id}`) };
    const del: Act | null = manage ? { id: 'delete', label: t('resources.act.delete'), icon: <Trash2 aria-hidden />, danger: true, run: () => {setTyped('');setAsked({ what: 'delete', agent });} } : null;
    return {
      primary: enable ?? open,
      edit,
      entries: entriesOf([open], [enable, disable], [switchThreads], [edit, rename], [share, copy], [del]),
      keys: keysOf(rename),
      busy: busy(agent.id),
    };
  };

  const bulk: BulkAct<Agent>[] = [
    {
      id: 'enable',
      label: t('resources.act.enable'),
      icon: <CirclePlay aria-hidden />,
      can: (agent) => holds(agent.access, 'agent.manage') && agent.disabled,
      open: (rows) => enablePlan.askMany(rows.map(enableResource)),
      done: (count) => t('agents.bulk.enabled', { count }),
    },
    {
      id: 'disable',
      label: t('resources.act.disable'),
      icon: <Ban aria-hidden />,
      can: (agent) => holds(agent.access, 'agent.manage') && !agent.disabled,
      run: (agent) => setDisabled(agent, true),
      confirm: (rows) => ({
        title: t('agents.bulk.disableTitle', { count: rows.length }),
        body: <BulkNames names={rows.map((row) => row.name)} lead={t('agents.danger.disableBody')} />,
        label: t('agents.bulk.disableConfirm', { count: rows.length }),
      }),
      done: (count) => t('agents.bulk.disabled', { count }),
    },
    {
      id: 'delete',
      label: t('resources.act.delete'),
      icon: <Trash2 aria-hidden />,
      danger: true,
      can: (agent) => holds(agent.access, 'agent.manage'),
      run: dropAgent,
      confirm: (rows) => ({
        title: t('agents.bulk.deleteTitle', { count: rows.length }),
        body: <BulkNames names={rows.map((row) => row.name)} lead={t('agents.danger.deleteBody')} />,
        label: t('agents.bulk.deleteConfirm', { count: rows.length }),
      }),
      done: (count) => t('agents.bulk.deleted', { count }),
    },
  ];

  const dialogs = (
    <>
      {enablePlan.dialogs}
      {asked !== null && (asked.what === 'edit' || asked.what === 'rename') && (
        <EditAgent
          key={`${asked.agent.id}-${asked.what}`}
          agent={asked.agent}
          {...(asked.what === 'rename' ? { only: 'name' as const } : {})}
          onProfile={() => {
            setAsked(null);
            options.onProfile?.(asked.agent);
          }}
          onClose={() => setAsked(null)}
        />
      )}
      <ConfirmDialog
        open={asked?.what === 'disable'}
        disabled={impact.isPending||impact.isError}
        admin={adminSpec(asked?.agent, 'agent.manage')}
        onOpenChange={(next) => !next && setAsked(null)}
        title={t('agents.danger.disableTitle', { name: asked?.agent.name ?? '' })}
        confirmLabel={t('agents.danger.disableConfirm', { name: asked?.agent.name ?? '' })}
        danger
        pending={toggle.isPending}
        error={toggle.error ?? undefined}
        onConfirm={() => asked !== null && toggle.mutate({ agent: asked.agent, disabled: true })}
      >
        <div className="flex flex-col gap-3">{impact.isPending?<SkeletonRows rows={2} />:impact.isError?<ErrorState compact error={impact.error} onRetry={()=>void impact.refetch()} />:<ImpactCounts impact={impact.data} kind="disable" />}<Input label={t('resourceImpact.note')} optional value={pauseNote} onChange={(event)=>setPauseNote(event.target.value)} /></div>
      </ConfirmDialog>
      <ConfirmDialog
        open={asked?.what === 'delete'}
        disabled={impact.isPending||impact.isError||typed!==(impact.data.name)}
        admin={adminSpec(asked?.agent, 'agent.manage')}
        onOpenChange={(next) => {
          if (next) return;
          setAsked(null);
          remove.reset();
        }}
        title={t('agents.danger.deleteTitle', { name: asked?.agent.name ?? '' })}
        confirmLabel={t('agents.danger.deleteConfirm', { name: asked?.agent.name ?? '' })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => asked !== null && remove.mutate(asked.agent)}
      >
        <div className="flex flex-col gap-2">
          {impact.isPending?<SkeletonRows rows={2} />:impact.isError?<ErrorState compact error={impact.error} onRetry={()=>void impact.refetch()} />:<ImpactCounts impact={impact.data} kind="delete" />}
          <span>{t('resourceImpact.deleteKeeps')}</span>
          <Button variant="ghost" size="sm" onClick={()=>{if(asked===null)return;const agent=asked.agent;setAsked(null);openChange({kind:'agent',id:agent.id,name:agent.name},{kind:'switch'});}}>{t('resourceImpact.switchFirst')}</Button>
          <Input label={t('resourceImpact.typeName',{name:impact.data?.name ?? asked?.agent.name})} value={typed} onChange={(event)=>setTyped(event.target.value)} autoComplete="off" />
        </div>
      </ConfirmDialog>
    </>
  );

  return {
    list: { name: (agent) => agent.name, menuName: (agent) => agent.name, of: (agent) => actsOf(agent, 'row'), bulk, dialogs },
    page: (agent) => {
      const acts = actsOf(agent, 'page');
      return { edit: acts.edit, primary: acts.primary, menu: acts.entries };
    },
    dialogs,
  };
}
