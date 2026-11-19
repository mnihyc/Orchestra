import { useEnvironmentFeed } from '@/realtime/environmentFeed';
/** The environment's MCP servers, their current states and masked failure output, with a manager's one-server Restart job. */
import { LibraryCounts } from '@/features/resources/Library';
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { RotateCw } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { useRetainedRows } from '@/lib/retainedRows';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Table } from '@/ui/Table';
import { Pagination } from '@/ui/Pagination';
import { EmptyState } from '@/ui/EmptyState';

import { environmentMcpPageQuery, environmentMcpQuery, restartEnvironmentMcpServer } from '@/api/environments';
import { jobQuery } from '@/api/forks';
import type { Environment, EnvironmentMcpServer } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';
import { HelpHint } from '@/ui/HelpHint';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { mcpReason, mcpReasonCode, mcpStateTone as TONE } from '@/features/resources/mcp/words';
import { ImportHere } from '@/features/resources/mcp/ImportHere';
import { FoundMcp } from '@/features/resources/mcp/Import';
import { repairEnvironmentMcp } from '@/api/mcpChanges';
import { EnvironmentMcpChoices } from '@/features/resources/mcp/Enabled';

type ServerRow = EnvironmentMcpServer & { id: string };
function Server({ environment, server, manage, visible, keep }: { environment: string; server: ServerRow; manage: boolean; visible: boolean; keep: (row: ServerRow, active: boolean) => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [jobId, setJobId] = useState<string | null>(null);
  const restart = useMutation({ mutationFn: () => restartEnvironmentMcpServer(environment, server.server.name), onSuccess: (job) => { setJobId(job.id); } });
  const progress = useQuery({ ...jobQuery(jobId ?? ''), enabled: jobId !== null });
  const state = progress.data?.state;
  const jobError = progress.data?.error ?? null;
  const pending = restart.isPending || (jobId !== null && (state === undefined || state === 'queued' || state === 'running'));
  useEffect(() => keep(server, pending), [keep, server, pending]);
  const finished = state === 'succeeded' || state === 'failed' || state === 'cancelled';
  const reason = mcpReason(server.reason, server.state, t, mcpReasonCode(server));
  useEffect(() => { if (finished) void repairEnvironmentMcp(client, environment); }, [finished, environment, client]);
  return <div hidden={!visible} className={!visible ? 'hidden' : 'flex min-w-0 flex-col gap-1 py-1'}>
    <div className="flex flex-wrap items-center gap-2">
      <ResourceChip subject={{ kind: 'mcp_server', server: server.server, state: server.state }} hint={false} />
      <StatusDot tone={TONE[server.state]} label={t(`mcpServers.states.${server.state}`)} showLabel />
      {manage && <Button size="sm" icon={<RotateCw aria-hidden />} title={t('mcpServers.restartHint')} pending={pending} disabled={pending || server.state === 'unavailable'} onClick={() => restart.mutate()}>{t('mcpServers.restart')}</Button>}
    </div>
    {server.library !== undefined && server.library !== null && <LibraryCounts facts={server.library}/>}
    {reason !== null && <div className="flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted" aria-label={t('mcpServers.failure')}>
      <span className="wrap-anywhere">{reason.text}</span>
      {reason.details !== null && <HelpHint term={t('mcpWords.details')} what={<pre className="whitespace-pre-wrap wrap-anywhere font-mono text-sm">{reason.details}</pre>} />}
    </div>}
    {pending && <p role="status" className="text-sm text-text-muted">{t('mcpServers.restarting')}</p>}
    {restart.error !== null && <ErrorAlert error={restart.error} />}
    {progress.error !== null && <ErrorAlert error={progress.error} />}
    {jobError !== null && <ErrorAlert error={jobError} />}
  </div>;
}

/** The same list is used in the thread's Environment panel and the environment's page. */
function Connections({ environment, compact = false, threadId }: { environment: Pick<Environment, 'id' | 'access' | 'status'> & Partial<Pick<Environment, 'name' | 'machine'>>; compact?: boolean; threadId?: string }) {
  const { t } = useTranslation();
  useEnvironmentFeed(environment.id);
  const search = useCollectionSearch('mcp-servers:' + environment.id);
  const current = useInfiniteQuery(environmentMcpPageQuery(environment.id, 'servers', search.q, threadId));
  const rows = useMemo(() => current.isError ? [] : current.data?.pages.flatMap((page) => page.servers).map((row) => ({ ...row, id: row.server.id })) ?? [], [current.data, current.isError]);
  const retained = useRetainedRows(rows);
  const facts = current.isError ? undefined : current.data?.pages.at(-1);
  const manage = holds(environment.access, 'environment.manage') && environment.status === 'running';
  return <section aria-label={t('mcpServers.title')} className={compact ? 'flex flex-col gap-3 border-b border-border px-3 py-3' : 'flex flex-col gap-4'}>
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1"><h3 className={compact ? 'text-xs font-medium text-text-muted' : 'font-medium'}>{t('resourceDetails.layout.mcpConnections')}</h3>
      {facts !== undefined && <span className="text-sm text-text-muted">{t(`mcpWords.applies.${facts.applies}`)}</span>}</div>
    {facts !== undefined && environment.name !== undefined && environment.machine !== null && environment.machine !== undefined && <CompleteImports threadId={threadId} environment={{ id: environment.id, name: environment.name }} machine={environment.machine} />}
    {facts !== undefined && (facts.found.length > 0 || (facts.connections?.length ?? 0) > 0) &&
      <FoundMcp key={JSON.stringify([environment.id, threadId])} environment={environment} machineId={environment.machine?.id} threadId={threadId} />}
    <CollectionSearch label={t('resourceDetails.mcp.list.search')} name={t('mcpServers.title')} {...search} />
    {current.error !== null ? <ErrorState compact error={current.error} onRetry={() => void current.refetch()} /> : current.data === undefined ? <SkeletonRows read={current} rows={2} /> : rows.length === 0 && (search.q !== '' || !facts?.found.length && !facts?.connections?.length) ? <EmptyState compact title={t(search.q === '' ? 'mcpServers.empty' : 'resourceDetails.mcp.list.noMatch')} /> : null}
    {retained.rows.length > 0 && <Table label={t('resourceDetails.layout.mcpConnections')} rows={retained.rows} rowKey={(row) => row.id} virtual retainKeys={retained.active}
      rowClassName={(row) => retained.visible.has(row.id) ? undefined : 'hidden'} className={rows.length === 0 ? 'hidden' : undefined}
      columns={[{ id: 'server', header: t('resourceDetails.layout.mcpConnections'), hideHeader: true, cell: (server) => <Server environment={environment.id} server={server} manage={manage} visible={retained.visible.has(server.id)} keep={retained.keep} /> }]} />}
    <Pagination hasMore={current.hasNextPage} loading={current.isFetchingNextPage} error={current.isFetchNextPageError ? current.error : null} onMore={() => void current.fetchNextPage()} />
    {environment.machine !== undefined && <EnvironmentMcpChoices environment={{ id: environment.id, machine: environment.machine }} />}
  </section>;
}

/** Import all uses a complete admitted input; it is independent of the searched connection rows. */
function CompleteImports({ environment, machine, threadId }: { environment: { id: string; name: string }; machine: NonNullable<Environment['machine']>; threadId?: string | undefined }) {
  const current = useQuery(environmentMcpQuery(environment.id, threadId));
  if (current.isError) return <ErrorState compact error={current.error} onRetry={() => void current.refetch()} />;
  return current.data === undefined ? null : <ImportHere environment={environment} machine={machine} value={current.data} threadId={threadId} />;
}

export function McpServers(props: Parameters<typeof Connections>[0]) {
  const { scope } = useCollectionSearch('mcp-servers:' + props.environment.id);
  return <Connections key={JSON.stringify([scope, props.threadId])} {...props} />;
}
