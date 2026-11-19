/** The environment's MCP servers, their current states and masked failure output, with a manager's one-server Restart job. */
import { LibraryCounts } from '@/features/resources/Library';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { RotateCw } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { environmentMcpKey, environmentMcpQuery, restartEnvironmentMcpServer } from '@/api/environments';
import { jobQuery } from '@/api/forks';
import type { Environment, EnvironmentMcpServer, McpState } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot, type DotTone } from '@/ui/StatusDot';
import { HelpHint } from '@/ui/HelpHint';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';

const TONE: Record<McpState, DotTone> = { starting: 'info', running: 'success', stopped: 'off', failed: 'danger', unavailable: 'warning' };

function Server({ environment, server, manage, compact }: { environment: string; server: EnvironmentMcpServer; manage: boolean; compact: boolean }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [jobId, setJobId] = useState<string | null>(null);
  const restart = useMutation({ mutationFn: () => restartEnvironmentMcpServer(environment, server.server.name), onSuccess: (job) => { setJobId(job.id); void client.invalidateQueries({ queryKey: environmentMcpKey(environment) }); } });
  const progress = useQuery({ ...jobQuery(jobId ?? ''), enabled: jobId !== null });
  const state = progress.data?.state;
  const jobError = progress.data?.error ?? null;
  const pending = restart.isPending || (jobId !== null && (state === undefined || state === 'queued' || state === 'running'));
  const finished = state === 'succeeded' || state === 'failed' || state === 'cancelled';
  useEffect(() => { if (finished) void client.invalidateQueries({ queryKey: environmentMcpKey(environment) }); }, [finished, environment, client]);
  return <li className={compact ? 'flex min-w-0 flex-col gap-1 py-1' : 'flex flex-col gap-2 rounded border border-border p-3'}>
    <div className="flex flex-wrap items-center gap-2">
      <ResourceChip subject={{ kind: 'mcp_server', server: server.server, state: server.state }} hint={false} />
      <StatusDot tone={TONE[server.state]} label={t(`mcpServers.states.${server.state}`)} showLabel />
      {manage && <Button size="sm" icon={<RotateCw aria-hidden />} title={t('mcpServers.restartHint')} pending={pending} disabled={pending || server.state === 'unavailable'} onClick={() => restart.mutate()}>{t('mcpServers.restart')}</Button>}
    </div>
    {server.library !== undefined && server.library !== null && <LibraryCounts facts={server.library}/>}
    {server.reason !== null && <p className="whitespace-pre-wrap break-words text-sm text-text-muted" aria-label={t('mcpServers.failure')}>{server.reason === 'No machine' ? t('mcpOffering.noMachine') : server.reason}</p>}
    {pending && <p role="status" className="text-sm text-text-muted">{t('mcpServers.restarting')}</p>}
    {restart.error !== null && <ErrorAlert error={restart.error} />}
    {progress.error !== null && <ErrorAlert error={progress.error} />}
    {jobError !== null && <ErrorAlert error={jobError} />}
  </li>;
}

/** The same list is used in the thread's Environment panel and the environment's page. */
export function McpServers({ environment, compact = false }: { environment: Pick<Environment, 'id' | 'access' | 'status'>; compact?: boolean }) {
  const { t } = useTranslation();
  const current = useQuery(environmentMcpQuery(environment.id));
  const manage = holds(environment.access, 'environment.manage') && environment.status === 'running';
  return <section aria-label={t('mcpServers.title')} className={compact ? 'flex flex-col gap-3 border-b border-border px-3 py-3' : 'flex flex-col gap-4'}>
    <div className="flex items-center gap-1"><h3 className={compact ? 'text-xs font-medium text-text-muted' : 'font-medium'}>{t('resourceDetails.layout.mcpConnections')}</h3>
      {compact && <HelpHint term={t('mcpServers.title')} what={t('mcpServers.applies')} />}</div>
    {!compact && <p className="text-sm text-text-muted">{t('mcpServers.applies')}</p>}
    {current.error !== null ? <ErrorState compact error={current.error} onRetry={() => void current.refetch()} /> : current.data === undefined ? <SkeletonRows rows={2} /> : current.data.servers.length === 0 ? <EmptyState compact title={t('mcpServers.empty')} /> : <ul className="flex flex-col gap-2">{current.data.servers.map((server) => <Server key={server.server.id} environment={environment.id} server={server} manage={manage} compact={compact} />)}</ul>}
  </section>;
}
