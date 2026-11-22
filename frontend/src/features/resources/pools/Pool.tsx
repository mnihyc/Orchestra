/** A pool keeps ordinary ownership, sharing, settings and its protected paged members. */
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { Layers3 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { poolQuery } from '@/api/pools';
import { openAccess } from '@/features/access/AccessDialog';
import { DetailSections } from '@/features/resources/DetailSections';
import { Facts } from '@/features/resources/Facts';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { useResourceHeading } from '@/features/resources/trail';
import { SettingsEditor } from '@/features/settings/Editor';
import { holds } from '@/lib/permissions';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { PoolExpanded } from './Expanded';
import { PoolMembers } from './Members';
import { DeletePool } from './Delete';
import { PoolHomeDialog } from './Home';
import { PoolDialog } from './PoolDialog';
import { PoolRecover } from './Recover';
import { PoolHistory } from './History';

export function PoolScreen({ id }: { id: string }) {
  const reader=useStore(client.session,(state)=>`${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  return <PoolPage key={`${id}:${reader}`} id={id} reader={reader} />;
}
function PoolPage({id,reader}:{id:string;reader:string}) {
  const { t } = useTranslation();
  const query = useQuery(poolQuery(id,reader));
  const [editing, setEditing] = useState(false);
  const [copying,setCopying]=useState(false);
  const [moving,setMoving]=useState(false);
  const [deleting,setDeleting]=useState(false);
  const navigate=useNavigate();
  const pool = query.data;
  const back = useResourceHeading('pools', Layers3, pool?.name, id);
  if (query.isPending) return <Page><SkeletonRows read={query} rows={4} /></Page>;
  if (query.isError || pool === undefined) return <Page><ErrorState error={query.error} onRetry={() => void query.refetch()} /></Page>;
  const target = { kind: 'pool', id, name: pool.name } as const;
  return <Page><PageHeader adminAccess={pool.access.admin_access} opening={{kind:'pool',id}} title={pool.name} back={back} mark={<PlaceMark><Layers3 aria-hidden /></PlaceMark>} ownership={pool}
    facts={[t(`pools.kinds.${pool.kind}`), t(pool.enabled ? 'pools.on' : 'pools.off'), t(pool.policy === null ? 'pools.policy.choose' : `pools.policy.${pool.policy.type}`)]}
    actions={<><Button size="sm" onClick={() => openAccess(target)}>{t('pools.share')}</Button><Button size="sm" disabled={!holds(pool.access,'pool.view')} title={holds(pool.access,'pool.view') ? undefined : t('duplicate.readRequired')} onClick={()=>setCopying(true)}>{t('duplicate.now')}</Button><Button size="sm" disabled={!holds(pool.access, 'pool.manage')} title={holds(pool.access, 'pool.manage') ? undefined : t('duplicate.manageRequired')} onClick={() => setEditing(true)}>{t('pools.editAct')}</Button><Button size="sm" disabled={!holds(pool.access, 'pool.manage')} onClick={() => setMoving(true)}>{t('pools.moveHome')}</Button></>} />
    <DetailSections label={pool.name} sections={[
      { value: 'members', label: t('pools.members'), panel: <PoolMembers pool={pool} /> },
      { value: 'expanded', label: t('pools.expanded'), panel: <PoolExpanded id={id} /> },
      { value: 'history', label: t('pools.history'), panel: <PoolHistory id={id} /> },
      { value: 'settings', label: t('pools.settings'), panel: <SettingsEditor target={{ kind: 'pool', id }} scope="pool" place={{ name: pool.name, owner: pool.owner, access: pool.access, kind: null }} compact /> },
      { value: 'access', label: t('pools.access'), panel: <ResourceAccess target={target} access={pool.access} /> },
      { value: 'details', label: t('common.details'), panel: <div className="flex flex-col gap-3"><Facts rows={[{ name: t('pools.lend'), value: t(pool.lend_members ? 'pools.on' : 'pools.off') }]} /><PoolRecover pool={pool} /><Button size="sm" disabled={!holds(pool.access, 'pool.manage')} onClick={() => setDeleting(true)}>{t('pools.delete')}</Button></div> },
    ]} />
    {deleting && <DeletePool pool={pool} onClose={() => setDeleting(false)} onDeleted={() => void navigate({to:'/resources/pools'})} />}
    {moving && <PoolHomeDialog pool={pool} onClose={() => setMoving(false)} />}
    {copying && <PoolDialog copy={pool} onClose={()=>setCopying(false)} onCreated={(id)=>void navigate({to:'/resources/pools/$id',params:{id}})} />}
    {editing && <PoolDialog pool={pool} onClose={() => setEditing(false)} />}
  </Page>;
}
