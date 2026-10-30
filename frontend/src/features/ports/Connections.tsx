/** Counts for Ports viewers and bounded live caller/inside-peer details for current managers. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { ActionIcon } from '@/ui/actions';
import { useViewChoice } from '@/lib/viewChoices';
import { connectionFactsQuery } from '@/api/ports';
import { ByteSize } from '@/ui/ByteSize';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { reachedAddress } from './model';
import { PortFault } from './Fault';
import type { PortOpening } from './presentation';
import type { PortCaller } from '@/generated/api-types';

/** The carrier keeps at most 128 recent rows per target/opening and a fifteen-minute window. */
const RECENT_ROWS = 128;
export function PortConnections({ opening, environmentId, port, count, manage }: { opening?: PortOpening; environmentId?: string; port?: number; count?: number | null; manage: boolean }) {
 const { t } = useTranslation(); const cache = useQueryClient(); const [inside, setInside] = useState('');
 const environment = opening?.environment_id ?? environmentId ?? ''; const openingId = opening?.id ?? null; const knownCount = opening?.connection_count ?? count ?? null;
 const [connectionsOpen, setConnectionsOpen] = useViewChoice<boolean>('ports', 'connections', false, `${environment}:${openingId ?? port}`);
 const selected = /^\d+$/.test(inside) && Number(inside)>=1 && Number(inside)<=65535 ? Number(inside) : undefined;
 const query=useQuery({ ...connectionFactsQuery(environment,openingId,selected,port),enabled:manage && connectionsOpen });
 const callerName=(caller:PortCaller|null)=>caller===null||caller.kind==='address'?t('ports.addressCaller'):caller.kind==='person'?caller.user.display_name:t('ports.linkCaller',{label:caller.label});
 useEffect(()=>{if(!manage) cache.removeQueries({queryKey:['port-connection-facts',environment,openingId]});},[cache,manage,environment,openingId]);
 if(!manage) return <span className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{knownCount===null?t('ports.countsUnknown'):t('ports.connections',{count:knownCount})}</span><span>{t('ports.callersManageOnly')}</span></span>;
 return <details open={connectionsOpen} onToggle={(event) => setConnectionsOpen(event.currentTarget.open)} className="rounded-md border border-border p-2"><summary className="cursor-pointer text-sm inline-flex items-center gap-2"><ActionIcon action={connectionsOpen ? 'hide' : 'show'} />{t('ports.connectionTitle')} ({query.data?.counts.active??knownCount??t('ports.unreported')})</summary><div className="mt-2 flex flex-col gap-2">
  <Input type="number" min={1} max={65535} label={t('ports.insidePort')} optional value={inside} onChange={(event)=>setInside(event.target.value)} />
  {query.isPending?<SkeletonRows rows={2}/>:query.isError?<PortFault error={query.error} onRefresh={()=>void query.refetch()}/>:<>
   <p className="text-xs text-text-muted">{t('ports.recentConnections',{limit:RECENT_ROWS})}</p>
   {query.data.connections===null?<EmptyState compact title={t('ports.callersManageOnly')} />:query.data.connections.length===0?<EmptyState compact title={t('ports.noConnections')} />:query.data.connections.map((connection)=><div key={connection.id} className="min-w-0 rounded-lg bg-bg-subtle p-2">
    <p className="flex flex-wrap gap-x-3 gap-y-1 break-all text-sm font-medium"><span>{callerName(connection.outside.caller)}</span><span>{reachedAddress(connection.outside.address,connection.outside.port)}</span></p>
    <p className="break-all text-xs text-text-muted">{t('ports.seenInside',{address:reachedAddress(connection.inside.address,connection.inside.port)})}</p>
    <p className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{t(connection.ended_at===null?'ports.connectionState.active':'ports.connectionState.closed')}</span><span>{connection.transport.toUpperCase()}</span>{connection.http_requests!==null && <span>{t('ports.connectionRequests',{count:connection.http_requests})}</span>}</p>
    <dl className="mt-1 grid grid-cols-2 gap-x-3 gap-y-1 text-xs text-text-muted"><div><dt>{t('ports.since')}</dt><dd><RelativeTime at={connection.since}/></dd></div><div><dt>{t('ports.lastActive')}</dt><dd><RelativeTime at={connection.last_active}/></dd></div><div><dt>{t('ports.received')}</dt><dd><ByteSize bytes={connection.bytes_in}/></dd></div><div><dt>{t('ports.sent')}</dt><dd><ByteSize bytes={connection.bytes_out}/></dd></div></dl>
   </div>)}
   {query.data.totals!==null&&query.data.totals.length>0&&<div className="flex flex-col gap-2"><h4 className="text-sm font-medium">{t('ports.callerTotals')}</h4>{query.data.totals.map((total,index)=><div key={index} className="rounded-lg border border-border p-2 text-xs"><strong>{total.other?t('ports.otherCallers'):callerName(total.caller)}</strong>{total.address!==null && <p>{total.address}</p>}<p>{t('ports.connections',{count:total.active})}</p><dl className="grid grid-cols-[auto_1fr] gap-x-3"><dt>{t('ports.received')}</dt><dd><ByteSize bytes={total.bytes_in}/></dd><dt>{t('ports.sent')}</dt><dd><ByteSize bytes={total.bytes_out}/></dd></dl>{total.http_requests!==null && <p>{t('ports.connectionRequests',{count:total.http_requests})}</p>}</div>)}</div>}
   {query.data.truncated&&<p className="text-xs text-warning-text">{t('ports.truncated')}</p>}
   <p className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{t('ports.liveMemory')}</span><span>{t('ports.trackedSince')} <RelativeTime at={query.data.counts.tracked_since}/></span><span>{t('ports.resetAt')} <RelativeTime at={query.data.counts.reset_at}/></span><span>{t('ports.recentWindow')}</span></p>
  </>}
 </div></details>;
}
