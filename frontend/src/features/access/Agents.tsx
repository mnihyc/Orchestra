/** Agents that can read or write this and why; share with agents by area (conversations, chats, documents), a thing or a whole place, as a copy or live, to read or to write, where editing a place's documents is also what makes them there; take either back.
 *
 * What is shared with agents is what they know beyond their own threads. This tab lists it: each share
 * with whose agents it is for, at what level in each area, until when, who shared it and since when, with the
 * ✕ that takes it back after saying what that does; on a place a second list says what its agents know from
 * above. "Share with agents…" opens the same form that sharing with a person is, with the same three
 * words and the same three reaches. Where a policy lets agents read their neighbours the tab says so first.
 */
import { useQuery } from '@tanstack/react-query';
import { Bot } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessTarget, isNodeKind, isPlaceKind, pageQuery } from '@/api/access';
import { meQuery } from '@/api/auth';
import { agentSharesQuery, canShareWithAgents } from '@/api/share';
import { settingsQuery } from '@/api/settings';
import type { AgentShare } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';

import { AgentRow } from './AgentRow';
import { CoversLine, type PlaceKind } from './Covers';
import { reachOf } from './agentShares';
import { KnownToAgents } from './KnownToAgents';
import { placeName } from './holders';
import { ShareWithAgentsDialog } from './ShareWithAgents';

/** The policy that lets agents read the threads beside them, with the place that set it: "The agents of this topic can read each other’s threads, by Acme’s setting." */
function ReachPolicy({ target }: { target: AccessTarget }) {
  const { t } = useTranslation();
  const settings = useQuery({ ...settingsQuery(target.id), enabled: isNodeKind(target.kind) && (target.kind === 'topic' || target.kind === 'thread') });
  const found = settings.data?.find((entry) => entry.key === 'sharing.agent_reach');
  if (found === undefined || (found.value !== 'topic' && found.value !== 'project')) return null;
  const place = found.source.node === null ? t('access.place.system') : placeName(t, found.source.node);
  return <InlineAlert tone="info">{t(found.value === 'topic' ? 'access.agents.policy.topic' : 'access.agents.policy.project', { place })}</InlineAlert>;
}

export function Agents({ target }: { target: AccessTarget }) {
  const { t } = useTranslation();
  const me = useQuery(meQuery).data;
  const page = useQuery(pageQuery(target));
  const shares = useQuery({ ...agentSharesQuery(target, 'of'), enabled: canShareWithAgents(target.kind) });
  const [sharing, setSharing] = useState(false);

  if (!canShareWithAgents(target.kind)) {
    return <EmptyState compact icon={<Bot aria-hidden />} title={t('access.agents.title')} description={t('access.agents.none', { name: target.name })} />;
  }
  if (shares.isError) return <ErrorState compact error={shares.error} title={t('access.agents.notLoaded')} retrying={shares.isRefetching} onRetry={() => void shares.refetch()} />;
  if (shares.isPending || page.isPending) return <SkeletonRows rows={4} />;

  const gives = page.data?.gives.agents ?? [];
  const list: AgentShare[] = shares.data;
  const place: PlaceKind | null = isPlaceKind(target.kind) ? target.kind : null;
  const reaches = new Set(list.filter((share) => share.mode === 'live').map(reachOf));

  return (
    <div className="flex flex-col gap-4">
      <ReachPolicy target={target} />
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-medium text-text">{t('access.agents.shared')}</h3>
        <Button
          variant="primary"
          icon={<Bot aria-hidden />}
          blocked={gives.length === 0}
          title={gives.length === 0 ? t('access.agents.cannotShare', { name: target.name }) : undefined}
          onClick={() => setSharing(true)}
        >
          {t('access.agents.share')}
        </Button>
      </div>

      {list.length === 0 ? (
        <EmptyState compact title={t('access.agents.nothing', { name: target.name })} />
      ) : (
        <ul aria-label={t('access.agents.shared')} className="flex flex-col divide-y divide-border rounded-lg border border-border">
          {list.map((share) => (
            <AgentRow key={share.id} share={share} target={target} gives={gives} meId={me?.id} />
          ))}
        </ul>
      )}

      {place !== null && page.data !== undefined && list.some((share) => share.mode === 'live') && (
        <div className="flex flex-col gap-1">
          {[...reaches].map((reach) => (
            <CoversLine key={reach} page={page.data} kind={place} name={target.name} reach={reach} whom="agents" />
          ))}
        </div>
      )}
      {list.length > 0 && <p className="text-sm text-text-muted">{t('access.agents.repeat')}</p>}

      {place !== null && <KnownToAgents target={target} />}
      {sharing && page.data !== undefined && <ShareWithAgentsDialog target={target} page={page.data} open onClose={() => setSharing(false)} />}
    </div>
  );
}
