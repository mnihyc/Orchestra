/** An agent's identity, its thread, environment and model, read only when its card opens. */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Bot } from 'lucide-react';
import { type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { nodeQuery } from '@/api/nodes';
import { agentQuery } from '@/api/resources';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import type { NodeRef } from '@/generated/api-types';
import { useThreadRow } from '@/realtime/stores/thread';
import { Avatar } from '@/ui/Avatar';
import { ErrorState } from '@/ui/ErrorState';
import { DetailCard } from '@/ui/DetailCard';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

export interface AgentIdentity {
  name: string;
  thread?: Pick<NodeRef, 'id' | 'name' | 'redacted'>;
  profileId?: string;
}

/** The same details can be placed in a kept card or read as an embedded identity. */
export function AgentDetails({ name, thread, profileId }: AgentIdentity) {
  const { t } = useTranslation();
  const query = useQuery({ ...nodeQuery(thread?.id ?? ''), enabled: thread !== undefined && !thread.redacted, retry: false });
  const profile = useQuery({ ...agentQuery(profileId ?? ''), enabled: thread === undefined && profileId !== undefined, retry: false });
  const live = useThreadRow(thread?.id ?? '');
  const current = live ?? (query.data?.kind === 'thread' ? query.data : null);
  const hidden = thread?.redacted === true || query.error instanceof ApiError && (query.error.status === 403 || query.error.status === 404);
  return <div className="flex max-w-[min(22rem,calc(100vw-2rem))] flex-col gap-3 p-3 text-text" data-testid="agent-card">
    <div className="flex items-center gap-3"><Avatar text="" color={0} kind="agent" size={40} mark={<Bot aria-hidden />} /><div><h3 className="font-semibold wrap-anywhere">{name}</h3><p className="text-sm text-text-muted">{t('agentIdentity.agent')}</p></div></div>
    {hidden ? <p className="text-sm text-text-muted">{t('agentIdentity.privateThread')}</p> : thread !== undefined ? query.error !== null ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : current === null ? <SkeletonRows rows={3} /> : <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
      <dt className="text-text-muted">{t('agentIdentity.owner')}</dt><dd><UserChip user={current.owner} size={24} /></dd>
      <dt className="text-text-muted">{t('agentIdentity.thread')}</dt><dd><Link to="/thread/$threadId" params={{ threadId: current.id }} className="text-link hover:underline">{current.name}</Link></dd>
      <dt className="text-text-muted">{t('agentIdentity.environment')}</dt><dd>{current.environment === null ? t('agentIdentity.noEnvironment') : current.environment_permissions.length > 0 ? <Link to="/resources/environments/$id" params={{ id: current.environment.id }} className="text-link hover:underline">{current.environment.name}</Link> : current.environment.name}</dd>
      <dt className="text-text-muted">{t('agentIdentity.model')}</dt><dd>{current.model===null?t('agentIdentity.noModel'):modelDisplayName(current.model_display,t)}</dd>
      {current.effort!==null && <><dt className="text-text-muted">{t('usage.chip.effort')}</dt><dd>{effortDisplayName(current.effort,t)}</dd></>}
      {current.binding !== null && <><dt className="text-text-muted">{t('agentIdentity.provider')}</dt><dd>{current.binding.provider.name}</dd></>}
    </dl> : profileId !== undefined ? profile.error !== null ? <ErrorState compact error={profile.error} onRetry={() => void profile.refetch()} /> : profile.data === undefined ? <SkeletonRows rows={2} /> : <><div className="text-sm">{profile.data.owner === null ? t('agentIdentity.systemProfile') : <UserChip user={profile.data.owner} />}</div><p className="text-sm text-text-muted">{t('agentIdentity.profileContext')}</p><Link to="/resources/agents/$id" params={{ id: profileId }} className="text-sm text-link hover:underline">{t('agentIdentity.openProfile')}</Link></> : <p className="text-sm text-text-muted">{t('agentIdentity.unknownThread')}</p>}
  </div>;
}

export function AgentCard({ children, ...identity }: AgentIdentity & { children: ReactNode }) {
  return <DetailCard label={identity.name} content={() => <AgentDetails {...identity} />} asChild>
    <button type="button" title={identity.name} aria-label={identity.name} className="inline-flex max-w-full min-w-0 rounded-sm text-left hover:underline" onClick={(event) => event.stopPropagation()}>{children}</button>
  </DetailCard>;
}
