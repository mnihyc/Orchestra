/** A thread's tool pair follows its source revision and explicit report reads. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { gatewayCatalogueQuery, threadGatewayQuery } from '@/api/gateways';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { IconButton } from '@/ui/IconButton';
import { HelpHint } from '@/ui/HelpHint';
import { TunnelGateway } from './TunnelGateway';

export function ThreadGatewayStatus({ threadId, executionRevision, line = false }: { threadId: string; executionRevision?: number | null; line?: boolean }) {
  const { t } = useTranslation();
  const status = useQuery({ ...threadGatewayQuery(threadId, executionRevision), staleTime: line ? Infinity : 0,
    retryOnMount: !line, refetchOnMount: line ? true : 'always' });
  const known = new Set(status.data?.plan === null || status.data?.plan === undefined ? []
    : [status.data.plan.preferred_gateway, ...status.data.plan.common_fallbacks].map((gateway) => gateway.id));
  const shownIds = status.data?.state === 'attached' && status.data.attached_gateway_id !== null ? [status.data.attached_gateway_id]
    : status.data?.state === 'unavailable' ? status.data.refusal?.attempted_gateways ?? [] : [];
  const catalogue = useQuery({ ...gatewayCatalogueQuery, enabled: !line || shownIds.some((id) => !known.has(id)) });
  if (status.isPending) return line ? null : <SkeletonRows rows={1} />;
  if (status.isError) return <ErrorState compact title={t('regionalGateways.statusFailed')} error={status.error} onRetry={() => void status.refetch()} />;
  if (line && status.data.state !== 'unavailable' && (status.data.state !== 'attached'
    || status.data.attached_gateway_id === null || status.data.attached_gateway_id === status.data.source_primary_gateway_id)) {
    return <ThreadGatewayDetails threadId={threadId} executionRevision={executionRevision} />;
  }
  return <div className="flex min-w-0 items-start justify-between gap-2">
    <TunnelGateway status={status.data} gateways={catalogue.data?.gateways ?? []} />
    <span className="inline-flex shrink-0 items-center gap-1">
      <HelpHint term={t('regionalGateways.tunnel')} what={t('regionalGateways.tunnelHelp')}
        guide={{ article: 'connect-a-machine', heading: 'tunnel-gateway' }} />
      <IconButton action="refresh" size="sm" label={t('regionalGateways.refresh')} pending={status.isFetching || catalogue.isFetching}
        onClick={() => { void status.refetch(); void catalogue.refetch(); }} />
    </span>
  </div>;
}

/** Environment thread lists read a pair on intent, avoiding a status request for every row. */
export function ThreadGatewayDetails({ threadId, executionRevision }: { threadId: string; executionRevision?: number | null }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)} className="min-w-0 text-xs text-text-muted">
    <summary className="focus-inset w-fit cursor-pointer rounded-sm">{t('regionalGateways.tunnel')}</summary>
    {open && <div className="min-w-0 pt-1"><ThreadGatewayStatus threadId={threadId} executionRevision={executionRevision} /></div>}
  </details>;
}
