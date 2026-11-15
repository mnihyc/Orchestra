/** Written rules stay paged; a member edit never replaces the pool's unloaded definition. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { poolMembersQuery, removePoolMember, readPoolAfterWrite } from '@/api/pools';
import type { Pool, PoolMemberRule, PoolMembersQuery, PoolMemberView } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Input } from '@/ui/Input';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { MemberDialog } from './MemberDialog';

export function PoolMembers({ pool }: { pool: Pool }) {
  const { t } = useTranslation();
  const [after, setAfter] = useState<PoolMembersQuery>({ after_id: null, after_position: null });
  const [text, setText] = useState('');
  const [editing, setEditing] = useState<PoolMemberView | 'new' | null>(null);
  const cache=useQueryClient();
  const remove=useMutation({mutationFn:(row:PoolMemberView)=>removePoolMember(pool.id,row.id,pool.revision),onSuccess:async ()=>{await readPoolAfterWrite(cache,pool.id);void query.refetch();}});
  const query = useQuery(poolMembersQuery(pool.id, after));
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
  const columns: Column<PoolMemberView>[] = [
    { id: 'rule', header: t('pools.member'), cell: (row) => <span>{row.label ?? label(row.rule)}</span> },
    { id: 'enabled', header: t('pools.enabled'), cell: (row) => t(row.enabled ? 'pools.on' : 'pools.off') },
    { id: 'priority', header: t('pools.priority'), cell: (row) => row.priority ?? t('pools.inherited') },
    { id: 'weight', header: t('pools.weight'), cell: (row) => row.weight ?? t('pools.inherited') },
    { id: 'edit', header: '', cell: (row) => <div className="flex gap-2"><Button size="sm" disabled={!manage} onClick={() => setEditing(row)}>{t('pools.editAct')}</Button><Button size="sm" disabled={!manage || remove.isPending} onClick={() => remove.mutate(row)}>{t('pools.removeMember')}</Button></div> },
  ];
  const rows = query.data?.items ?? [];
  const matching = rows.filter((row) => label(row.rule).toLocaleLowerCase().includes(text.trim().toLocaleLowerCase()));
  return <div className="flex min-w-0 flex-col gap-3">
    <div className="flex items-center gap-2"><Input label={t('pools.searchMembers')} hideLabel value={text} onChange={(event) => setText(event.target.value)} />
      <Button size="sm" disabled={!manage} onClick={() => setEditing('new')}>{t('pools.addMember')}</Button></div>
    {query.isPending ? <SkeletonRows read={query} rows={3} /> : query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> :
      <div className="max-h-[60vh] overflow-auto">{matching.length === 0 && <span>{t('pools.noMembers')}</span>}<Table label={t('pools.members')} columns={columns} rows={matching} rowKey={(row) => row.id} /></div>}
    {remove.isError && <ErrorState compact error={remove.error} onRetry={() => remove.reset()} />}
    <div className="flex justify-between gap-2"><Button size="sm" disabled={query.isFetching || after.after_id === null} onClick={() => setAfter({ after_id: null, after_position: null })}>{t('documentFolders.beginning')}</Button>
      <Button size="sm" disabled={query.isFetching || (query.data === undefined || query.data.next === null)} onClick={() => { if ((query.data !== undefined && query.data.next !== null)) setAfter(query.data.next); }}>{t('documentFolders.next')}</Button></div>
    {editing !== null && <MemberDialog pool={pool} member={editing === 'new' ? undefined : editing} onClose={() => setEditing(null)} onSaved={() => void query.refetch()} />}
  </div>;
}
