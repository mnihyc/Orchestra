import { useEnvironmentFeed } from '@/realtime/environmentFeed';
/** Discoveries are reviewed and imported through their actual environment; private definitions stay on the machine. */
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { environmentMcpPageQuery, environmentsQuery, importEnvironmentMcp } from '@/api/environments';
import { mcpKey } from '@/api/resource-tools';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { EnvironmentMcp, EnvironmentRef, McpFound } from '@/generated/api-types';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { Facts } from '@/features/resources/Facts';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { Pagination } from '@/ui/Pagination';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { Table, type Column } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { mcpProgram, mcpStateTone } from './words';
import { StatusDot } from '@/ui/StatusDot';
import { discoveryKey, validMcpName } from './importValues';
import { useImportState } from './ImportState';
export { discoveryKey, validMcpName } from './importValues';

export const NO_ENVIRONMENT = '\u0000choose-mcp-environment';
export const discoveryProgram = mcpProgram;

interface ProgramMcpRow {
  key: string;
  found: McpFound | null;
  name: string;
  source: string;
  directory: string | null;
  scope: McpFound['scope'] | null;
  transport: McpFound['transport'] | null;
  imported: string | null;
  profile: McpFound['profile'];
  state: McpFound['state'];
  using: McpFound['using'];
}

function programRows(value: EnvironmentMcp): ProgramMcpRow[] {
  const rows: ProgramMcpRow[] = value.found.map((found) => ({ ...found, profile: found.profile, state: found.state, using: found.using, key: discoveryKey(found), found }));
  for (const connection of value.connections ?? []) {
    const matching = rows.filter((row) => row.name === connection.name && row.source === connection.source);
    if (matching.length === 0) rows.push({ ...connection, key: JSON.stringify([connection.source, connection.name]), found: null,
      directory: null, scope: null, transport: null, imported: null, profile: undefined });
    else for (const row of matching) { row.state = connection.state; row.using = connection.using; }
  }
  return rows;
}

export function ImportMcp({ environment, found, right, onClose, onImported, machineId }: {
  environment: Pick<EnvironmentRef, 'id' | 'name'>; found: McpFound; right: EnvironmentMcp['import']; onClose: () => void; onImported: (id: string) => void; machineId?: string;
}) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const actKey = useActKey();
  const state = useImportState(environment.id, machineId ?? null);
  const [name, setName] = useState(found.name);
  const [enable, setEnable] = useState(true);
  const wrong = !validMcpName(name.trim());
  const request = { ...(found.origin === undefined || found.origin === null ? {} : { origin: found.origin }), source: found.source, scope: found.scope, name: found.name, directory: found.directory,
    ...(name.trim() === found.name ? {} : { as: name.trim() }), enable };
  const create = useMutation({
    mutationFn: async () => {
      const reader = libraryReader();
      const made = await importEnvironmentMcp(environment.id, request, actKey(request));
      return { made, reader };
    },
    onSuccess: ({ made, reader }) => {
      if (!sameLibraryReader(reader)) return;
      state.acknowledge(found, made.id);
      if (made.server !== null) cache.setQueryData(mcpKey(made.id), made.server);
      void cache.invalidateQueries({ queryKey: ['mcp-servers'] });
      toast({ tone: 'success', title: t('mcpImport.imported') });
      onImported(made.id);
      onClose();
    },
  });
  const reason = right.allowed ? undefined : t(right.reason === 'not_owner' ? 'mcpImport.notOwner' : right.reason === 'policy' ? 'mcpImport.policyOff' : 'mcpImport.unavailable');
  return <Dialog open onOpenChange={(open) => { if (!open && !create.isPending) onClose(); }} title={t('mcpImport.title')} dismissible={!create.isPending}
    footer={<><Button variant="ghost" disabled={create.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" pending={create.isPending} blocked={wrong || !right.allowed} title={reason} onClick={() => create.mutate()}>{t('mcpImport.import')}</Button></>}>
    <div className="flex min-w-0 flex-col gap-3">
      <Input label={t('mcpImport.name')} value={name} disabled={create.isPending} error={wrong ? t('mcpImport.nameInvalid') : undefined} onChange={(event) => setName(event.target.value)} />
      <Facts compact rows={[
        { name: t('mcpImport.environment'), value: environment.name },
        { name: t('mcpImport.program'), value: found.profile === undefined || found.profile === null ? discoveryProgram(found.source) : <ResourceChip subject={{ kind: 'agent', agent: found.profile }} hint={false} /> },
        { name: t('mcpImport.scope'), value: t(`mcpImport.scopes.${found.scope}`) },
        { name: t('mcpImport.transport'), value: t(`mcpImport.transports.${found.transport}`) },
        { name: t('mcpImport.keeps'), value: t(`mcpImport.shape.${found.transport}`) },
        { name: t('mcpImport.secrets'), value: t('mcpImport.stayOnMachine') },
      ]} />
      <Switch label={t('mcpImport.enableHere')} checked={enable} disabled={create.isPending || !right.allowed} onCheckedChange={setEnable} />
      {reason !== undefined && <span className="text-sm text-text-muted">{reason}</span>}
      {create.error !== null && <ErrorAlert error={create.error} />}
    </div>
  </Dialog>;
}

export function FoundMcp({ environment, machineId, threadId }: { environment: Pick<EnvironmentRef, 'id'> & Partial<Pick<EnvironmentRef, 'name'>>; machineId?: string | undefined; threadId?: string | undefined }) {
  const { t } = useTranslation();
  useEnvironmentFeed(environment.id);
  const search = useCollectionSearch('mcp-found:' + environment.id);
  const current = useInfiniteQuery(environmentMcpPageQuery(environment.id, 'found', search.q, threadId));
  const [chosen, setChosen] = useState<McpFound | null>(null);
  const state = useImportState(environment.id, machineId ?? null);
  const found = current.data?.pages.flatMap(programRows) ?? [];
  const managedNames = new Set(current.data?.pages.flatMap((page) => page.servers.map((row) => row.server.name)));
  const right = current.data?.pages.at(-1)?.import ?? { allowed: false, reason: null };
  const scopeLabel = (row: ProgramMcpRow) => row.scope === null ? t('mcpImport.notReported') : t(`mcpImport.scopes.${row.scope}`);
  const reason = !right.allowed ? t(right.reason === 'not_owner' ? 'mcpImport.notOwner' : right.reason === 'policy' ? 'mcpImport.policyOff' : 'mcpImport.unavailable') : environment.name === undefined ? t('mcpImport.unavailable') : undefined;
  const imported = (row: ProgramMcpRow) => row.imported ?? (row.found === null ? null : state.accepted[discoveryKey(row.found)]) ?? null;
  const columns: Column<ProgramMcpRow>[] = [
    { id: 'name', header: t('resources.listName'), cell: (row, shown) => {
      const id = row.imported;
      const scopeShown = shown.has('scope');
      const using = row.using === 'native' ? t('mcpImport.usingNative') : row.using === 'orch' ? t('mcpImport.usingOrch') : row.imported !== null || managedNames.has(row.name) ? t('mcpImport.definitionNotReported') : null;
      return <div className="flex min-w-0 flex-col gap-1">
        {id === null ? <span className="wrap-anywhere font-medium">{row.name}</span> : <ResourceChip subject={{ kind: 'mcp_server', server: { id, name: row.name } }} hint={false} />}
        {!scopeShown && <span className="text-sm text-text-muted">{scopeLabel(row)}</span>}
        {row.directory !== null && <span className="wrap-anywhere text-sm text-text-muted">{row.directory}</span>}
        {using !== null && <span className="text-sm text-text-muted">{using}</span>}
      </div>;
    } },
    { id: 'state', header: t('mcpImport.state'), cell: (row) => <StatusDot tone={row.state === undefined || row.state === null ? 'off' : mcpStateTone[row.state]} label={row.state === undefined || row.state === null ? t('mcpImport.notReported') : t(`mcpServers.states.${row.state}`)} showLabel /> },
    { id: 'program', header: t('mcpImport.program'), cell: (row) => row.profile === undefined || row.profile === null ? discoveryProgram(row.source) : <ResourceChip subject={{ kind: 'agent', agent: row.profile }} hint={false} /> },
    { id: 'scope', header: t('mcpImport.scope'), from: 'sm', cell: scopeLabel },
    { id: 'transport', header: t('mcpImport.transport'), from: 'md', cell: (row) => row.transport === null ? t('mcpImport.notReported') : t(`mcpImport.transports.${row.transport}`) },
    { id: 'act', header: t('mcpImport.import'), hideHeader: true, align: 'end', cell: (row) => row.found === null ? <span className="text-sm text-text-muted">{t('mcpImport.configurationNotReported')}</span> : imported(row) !== null ? <span className="text-sm text-text-muted">{t('mcpImport.imported')}</span>
      : <div className="flex flex-col items-end gap-1"><Button size="sm" blocked={reason !== undefined} title={reason} onClick={() => setChosen(row.found)}>{t('mcpImport.import')}</Button>{reason !== undefined && <span className="text-sm text-text-muted">{reason}</span>}</div> },
  ];
  return <section className="flex min-w-0 flex-col gap-3" aria-label={t('mcpImport.found')}>
    <h3 className="font-medium">{t('mcpImport.found')}</h3>
    <Input type="search" label={t('mcpImport.search')} value={search.text} onChange={(event) => search.onText(event.target.value)} />
    {current.isPending ? <SkeletonRows read={current} rows={3} /> : current.isError && !current.isFetchNextPageError ? <ErrorState compact error={current.error} onRetry={() => void current.refetch()} /> : found.length === 0 ? <EmptyState compact title={t(search.q !== '' ? 'mcpImport.noMatches' : right.reason === 'policy' ? 'mcpImport.policyOff' : 'mcpImport.empty')} /> : <Table label={t('mcpImport.found')} columns={columns} rows={found} rowKey={(row) => row.key} virtual key={search.q} />}
    <Pagination hasMore={current.hasNextPage} loading={current.isFetchingNextPage} error={current.isFetchNextPageError ? current.error : null} onMore={() => void current.fetchNextPage()} />
    {chosen !== null && environment.name !== undefined && <ImportMcp environment={{ id: environment.id, name: environment.name }} machineId={machineId} found={chosen} right={right} onClose={() => setChosen(null)}
      onImported={(id) => state.acknowledge(chosen, id)} />}
  </section>;
}

export function ImportFromMachine({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const environments = useQuery(environmentsQuery(null));
  const [selected, setSelected] = useState('');
  const rows = environments.data?.filter((environment) => environment.machine !== null && holds(environment.access, 'environment.use')) ?? [];
  const environment = rows.find((row) => row.id === selected);
  return <Dialog open title={t('mcpImport.fromMachine')} onOpenChange={(open) => { if (!open) onClose(); }} size="lg" footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
    <div className="flex min-w-0 flex-col gap-4">
      {environments.isPending ? <SkeletonRows rows={3} /> : environments.isError ? <ErrorState compact error={environments.error} onRetry={() => void environments.refetch()} />
        : rows.length === 0 ? <EmptyState compact title={t('mcpImport.noEnvironment')} /> : <>
          <ResourcePicker pickerKey="mcp-import.environment" label={t('mcpImport.environment')} value={environment?.id ?? NO_ENVIRONMENT} onValueChange={setSelected}
            options={[{ value: NO_ENVIRONMENT, label: t('mcpImport.chooseEnvironment'), disabled: true }, ...rows.map((row) => ({ value: row.id, label: `${row.machine?.name ?? ''} / ${row.name}` }))]} />
          {environment !== undefined && <FoundMcp key={environment.id} environment={environment} machineId={environment.machine?.id} />}
        </>}
    </div>
  </Dialog>;
}
