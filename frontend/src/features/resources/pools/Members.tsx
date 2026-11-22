/** Written rules stay paged; a member edit never replaces the pool's unloaded definition. */
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { poolMembersQuery, removePoolMember, readPoolAfterWrite } from '@/api/pools';
import type { Pool, PoolMemberRule, PoolMembersQuery, PoolMemberView } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Pencil, Trash2 } from 'lucide-react';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { MemberDialog } from './MemberDialog';

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
  const label = (rule: PoolMemberRule | null): string => {
    if (rule === null) return t('pools.hidden');
    switch (rule.type) {
      case 'single': return rule.resource.kind === 'model' ? rule.resource.backend.offering?.offering ?? t('pools.hidden')
        : rule.resource.kind === 'environment_template' ? t('pools.rules.template_on_host') : t('pools.rules.single');
      case 'pool': return t('pools.rules.pool');
      case 'provider_models': return rule.group === null ? t('pools.rules.provider_models') : t('pools.groupModels', { name: rule.group });
      case 'grouping': return t(`pools.groups.${rule.group.by}`);
      default: return t(`pools.rules.${rule.type}`);
    }
  };
  // A provider's models are named by their provider, whose name the server gives beside the rule.
  const named = (row: PoolMemberView): string => row.rule?.type === 'provider_models' && row.label !== null
    ? row.rule.group === null ? t('pools.providerModelsOf', { provider: row.label }) : t('pools.groupModelsOf', { name: row.rule.group, provider: row.label })
    : row.label ?? label(row.rule);
  const enabled = (row: PoolMemberView) => t(row.enabled ? 'pools.on' : 'pools.off');
  const priority = (row: PoolMemberView) => row.priority ?? t('pools.inherited');
  const weight = (row: PoolMemberView) => row.weight ?? t('pools.inherited');
  // Where the page is narrow, the columns it leaves out are said under the member's name; the acts never leave the row.
  const folded = (row: PoolMemberView, shown: ReadonlySet<string>): string | null => shown.has('priority') ? null : [
    ...(shown.has('enabled') ? [] : [enabled(row)]), `${t('pools.priority')} ${String(priority(row))}`, `${t('pools.weight')} ${weight(row)}`,
  ].join(' · ');
  const columns: Column<PoolMemberView>[] = [
    { id: 'rule', header: t('pools.member'), cell: (row, shown) => {
      const facts = folded(row, shown);
      return <div className="flex min-w-0 flex-col"><span>{named(row)}</span>{facts !== null && <span className="text-xs text-text-muted">{facts}</span>}</div>;
    } },
    { id: 'enabled', header: t('pools.enabled'), from: 'sm', cell: enabled },
    { id: 'priority', header: t('pools.priority'), from: 'md', cell: priority },
    { id: 'weight', header: t('pools.weight'), from: 'md', cell: weight },
    { id: 'edit', header: t('admin.users.actions'), hideHeader: true, align: 'end', cell: (row) => <div className="flex gap-1">
      <IconButton size="sm" icon={<Pencil aria-hidden />} label={t('pools.editMember', { name: named(row) })} disabled={!manage} onClick={() => setEditing(row)} />
      <IconButton size="sm" icon={<Trash2 aria-hidden />} label={t('pools.removeMemberNamed', { name: named(row) })} disabled={!manage || remove.isPending} onClick={() => remove.mutate(row)} />
    </div> },
  ];
  const rows = query.data?.items ?? [];
  const matching = rows.filter((row) => named(row).toLocaleLowerCase().includes(text.trim().toLocaleLowerCase()));
  return <div className="flex min-w-0 flex-col gap-3">
    <div className="flex items-center gap-2"><Input label={t('pools.searchMembers')} hideLabel value={text} onChange={(event) => setText(event.target.value)} />
      <Button size="sm" disabled={!manage} onClick={() => setEditing('new')}>{t('pools.addMember')}</Button></div>
    {query.isPending ? <SkeletonRows read={query} rows={3} /> : query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> :
      <div className="max-h-[60vh] overflow-auto">{matching.length === 0 && <span>{t('pools.noMembers')}</span>}<Table label={t('pools.members')} columns={columns} rows={matching} rowKey={(row) => row.id} /></div>}
    {remove.isError && <ErrorState compact error={remove.error} onRetry={() => remove.reset()} />}
    <div className="flex justify-between gap-2"><Button size="sm" disabled={query.isFetching || after.after_id === null} onClick={() => setAfter({ after_id: null, after_position: null })}>{t('documentFolders.beginning')}</Button>
      <Button size="sm" disabled={query.isFetching || next === null} onClick={() => { if (next !== null) setAfter(next); }}>{t('documentFolders.next')}</Button></div>
    {editing !== null && <MemberDialog pool={pool} member={editing === 'new' ? undefined : editing} onClose={() => setEditing(null)} onSaved={() => void query.refetch()} />}
  </div>;
}
