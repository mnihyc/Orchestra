/** Authorized host facts carried by a blocked plan; unknown or malformed extras do not invent a host. */
import {useTranslation} from 'react-i18next';
import {ApiError} from '@/api/client';
import {RelativeTime} from '@/ui/RelativeTime';

export function planHostBlockers(error: unknown) {
  if (!(error instanceof ApiError) || !error.is('plan.blocked')) return [];
  const entries=error.problem.host_blockers;
  const hosts=Array.isArray(entries)?entries.flatMap((entry:unknown)=>{
    if (typeof entry!=='object' || entry===null || !('host' in entry) || !('online' in entry) || typeof entry.online!=='boolean' || !('message' in entry) || typeof entry.message!=='string') return [];
    const host:unknown=entry.host;
    if (typeof host!=='object' || host===null || !('id' in host) || typeof host.id!=='string' || !('name' in host) || typeof host.name!=='string') return [];
    const since='offline_since' in entry && typeof entry.offline_since==='string' && Number.isFinite(Date.parse(entry.offline_since))?entry.offline_since:null;
    return [{id:host.id,name:host.name,online:entry.online,message:entry.message,since}];
  }):[];
  return hosts;
}

export function PlanHostBlockers({error}:{error:unknown}) {
  const {t}=useTranslation();
  const hosts=planHostBlockers(error);
  if (hosts.length===0) return null;
  return <ul aria-label={t('planHosts.title')} className="mt-2 flex flex-col gap-2 text-left text-sm">
    {hosts.map((host)=><li key={host.id} className="wrap-anywhere">
      {host.online?host.message:<><span className="font-medium">{host.name}</span>{' · '}<span>{t('planHosts.offline')}</span>{' · '}{host.since===null?t('planHosts.unknownSince'):<>{t('planHosts.since')}{' '}<RelativeTime at={host.since} /></>}</>}
    </li>)}
  </ul>;
}
