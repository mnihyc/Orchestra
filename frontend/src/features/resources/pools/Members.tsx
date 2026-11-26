/** Written rules stay paged; a member edit never replaces the pool's unloaded definition.
 *
 * Each row names its member as a person knows it: the resource's own name (the provider whose models it takes, the
 * host it places on) and, quietly beside it, what of it the rule takes. A member's acts are its "⋯" menu, the same
 * list as the row's context menu, so the table fits the pool's page at every width; the priority and weight move
 * into the member's own lines where their columns have no room. */
import { MoreHorizontal } from 'lucide-react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { poolMembersQuery, removePoolMember, readPoolAfterWrite } from '@/api/pools';
import type { Pool, PoolMembersQuery, PoolMemberView } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { MemberDialog } from './MemberDialog';

/** The member as a person knows it: the name of what it takes, and what of it, in a quieter word. A readable rule
 * whose resource has no name to give (it was removed) says what it takes instead. */
export function memberWords(row: Pick<PoolMemberView, 'label' | 'rule'>, t: TFunction): { name: string; kind: string | null } {
  const { rule, label } = row;
  if (rule === null) return { name: label ?? t('pools.hidden'), kind: null };
  const named = (what: string, kind: string | null) => label === null ? { name: what, kind: null } : { name: label, kind };
  switch (rule.type) {
    case 'single':
      if (rule.resource.kind === 'model') return { name: rule.resource.backend.offering?.offering ?? t('pools.exactModel'), kind: label };
      if (rule.resource.kind === 'environment_template') return named(t('pools.rules.template_on_host'), t('pools.rules.template_on_host'));
      return named(t('pools.rules.single'), null);
    case 'pool': return named(t('pools.rules.pool'), t('pools.rules.pool'));
    case 'provider_models': {
      const taken = rule.group === null ? t('pools.allModels') : t('pools.groupModels', { name: rule.group });
      return named(rule.group === null ? t('pools.rules.provider_models') : taken, taken);
    }
    case 'host_environments': return named(t('pools.rules.host_environments'), t('pools.rules.host_environments'));
    case 'template_on_host': return named(t('pools.rules.template_on_host'), t('pools.rules.template_on_host'));
    case 'grouping': return { name: t(`pools.groups.${rule.group.by}`), kind: null };
    case 'matching': return { name: t('pools.rules.matching'), kind: null };
  }
}

export function PoolMembers({ pool }: { pool: Pool }) {
  const reader=useStore(client.session,(state)=>`${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  return <Members key={`${pool.id}:${reader}`} pool={pool} reader={reader} />;
}
function Members({pool,reader}:{pool:Pool;reader:string}) {
  const { t } = useTranslation();
  const [after, setAfter] = useState<PoolMembersQuery>({ after_id: null, after_position: null });
  const [text, setText] = useState('');
  const [editing, setEditing] = useState<PoolMemberView | 'new' | null>(null);
  const cache=useQueryClient();
  const remove=useMutation({mutationFn:(row:PoolMemberView)=>removePoolMember(pool.id,row.id,pool.revision),onSuccess:async ()=>{await readPoolAfterWrite(cache,pool.id);void query.refetch();}});
  const query = useQuery(poolMembersQuery(pool.id, after,reader));
  const next=query.isError ? null : query.data?.next ?? null;
  const manage = holds(pool.access, 'pool.manage');
  const refusal = manage ? false : t('resources.act.manageRequired');
  const entries = (row: PoolMemberView): MenuEntry[] => [
    { id: 'edit', label: t('pools.editMember'), disabled: refusal, onSelect: () => setEditing(row) },
    { id: 'remove', label: t('pools.removeMember'), danger: true, disabled: refusal, onSelect: () => remove.mutate(row) },
  ];
  const priority = (row: PoolMemberView) => row.priority ?? t('pools.inherited');
  const weight = (row: PoolMemberView) => row.weight ?? t('pools.inherited');
  const columns: Column<PoolMemberView>[] = [
    { id: 'name', header: t('pools.member'), grow: true, cell: (row, shown) => {
      const { name, kind } = memberWords(row, t);
      // Where the room leaves out a column, what it said joins the member's own quiet line.
      const left = [
        ...(shown.has('enabled') ? [] : row.enabled ? [] : [t('pools.off')]),
        ...(shown.has('priority') || row.priority === null ? [] : [t('pools.priorityValue', { value: row.priority })]),
        ...(shown.has('weight') || row.weight === null ? [] : [t('pools.weightValue', { value: row.weight })]),
      ];
      const quiet = [...(kind === null ? [] : [kind]), ...left].join(' · ');
      return <div className="flex min-w-0 flex-col">
        <span className="truncate" title={name}>{name}</span>
        {quiet !== '' && <span className="truncate text-sm text-text-muted" title={quiet}>{quiet}</span>}
      </div>;
    } },
    { id: 'enabled', header: t('pools.enabled'), from: 'sm', cell: (row) => t(row.enabled ? 'pools.on' : 'pools.off') },
    { id: 'priority', header: t('pools.priority'), from: 'md', cell: priority },
    { id: 'weight', header: t('pools.weight'), from: 'md', cell: weight },
    { id: 'acts', header: t('operation.actions'), hideHeader: true, align: 'end', cell: (row) => <Menu label={memberWords(row, t).name} entries={entries(row)}
      trigger={<IconButton label={t('operation.actions')} icon={<MoreHorizontal aria-hidden />} />} /> },
  ];
  const rows = query.data?.items ?? [];
  const wanted = text.trim().toLocaleLowerCase();
  const matching = rows.filter((row) => { const { name, kind } = memberWords(row, t); return `${name} ${kind ?? ''}`.toLocaleLowerCase().includes(wanted); });
  return <div className="flex min-w-0 flex-col gap-3">
    <div className="flex min-w-0 items-center gap-2"><div className="min-w-0 flex-1"><Input label={t('pools.searchMembers')} hideLabel placeholder={t('pools.searchMembers')} value={text} onChange={(event) => setText(event.target.value)} /></div>
      <Button size="sm" disabled={!manage} onClick={() => setEditing('new')}>{t('pools.addMember')}</Button></div>
    {query.isPending ? <SkeletonRows read={query} rows={3} /> : query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> :
      matching.length === 0 ? <EmptyState compact title={t('pools.noMembers')} />
        : <Table label={t('pools.members')} columns={columns} rows={matching} rowKey={(row) => row.id} boundedIdentity rowMenu={{ label: (row) => memberWords(row, t).name, entries }} />}
    {remove.isError && <ErrorState compact error={remove.error} onRetry={() => remove.reset()} />}
    {(after.after_id !== null || next !== null) && <div className="flex justify-between gap-2"><Button size="sm" disabled={query.isFetching || after.after_id === null} onClick={() => setAfter({ after_id: null, after_position: null })}>{t('documentFolders.beginning')}</Button>
      <Button size="sm" disabled={query.isFetching || next === null} onClick={() => { if (next !== null) setAfter(next); }}>{t('documentFolders.next')}</Button></div>}
    {editing !== null && <MemberDialog pool={pool} member={editing === 'new' ? undefined : editing} onClose={() => setEditing(null)} onSaved={() => void query.refetch()} />}
  </div>;
}
