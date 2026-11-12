/** Discoveries are reviewed and imported through their actual environment; private definitions stay on the machine. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { environmentKey, environmentMcpKey, environmentMcpQuery, environmentsQuery, importEnvironmentMcp } from '@/api/environments';
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
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { Table, type Column } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { mcpProgram } from './words';
import { discoveryKey, validMcpName } from './importValues';
import { useImportState } from './ImportState';
export { discoveryKey, validMcpName } from './importValues';

export const NO_ENVIRONMENT = '\u0000choose-mcp-environment';
export const discoveryProgram = mcpProgram;

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
  const request = { source: found.source, scope: found.scope, name: found.name, directory: found.directory,
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
      void cache.invalidateQueries({ queryKey: environmentMcpKey(environment.id) });
      void cache.invalidateQueries({ queryKey: environmentKey(environment.id), exact: true });
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
        { name: t('mcpImport.program'), value: discoveryProgram(found.source) },
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

export function FoundMcp({ environment, machineId }: { environment: Pick<EnvironmentRef, 'id' | 'name'>; machineId?: string }) {
  const { t } = useTranslation();
  const current = useQuery(environmentMcpQuery(environment.id));
  const [chosen, setChosen] = useState<McpFound | null>(null);
  const state = useImportState(environment.id, machineId ?? null);
  if (current.isError) return <ErrorState compact error={current.error} retrying={current.isRefetching} onRetry={() => void current.refetch()} />;
  if (current.data === undefined) return <SkeletonRows rows={3} />;
  const { found, import: right } = current.data;
  const reason = right.allowed ? undefined : t(right.reason === 'not_owner' ? 'mcpImport.notOwner' : right.reason === 'policy' ? 'mcpImport.policyOff' : 'mcpImport.unavailable');
  const imported = (row: McpFound) => row.imported ?? state.accepted[discoveryKey(row)] ?? null;
  const columns: Column<McpFound>[] = [
    { id: 'name', header: t('resources.listName'), cell: (row) => {
      const id = row.imported;
      return id === null ? <span className="wrap-anywhere font-medium">{row.name}</span> : <ResourceChip subject={{ kind: 'mcp_server', server: { id, name: row.name } }} hint={false} />;
    } },
    { id: 'program', header: t('mcpImport.program'), cell: (row) => discoveryProgram(row.source) },
    { id: 'scope', header: t('mcpImport.scope'), from: 'sm', cell: (row) => t(`mcpImport.scopes.${row.scope}`) },
    { id: 'transport', header: t('mcpImport.transport'), from: 'md', cell: (row) => t(`mcpImport.transports.${row.transport}`) },
    { id: 'act', header: t('mcpImport.import'), hideHeader: true, align: 'end', cell: (row) => imported(row) !== null ? <span className="text-sm text-text-muted">{t('mcpImport.imported')}</span>
      : <div className="flex flex-col items-end gap-1"><Button size="sm" blocked={!right.allowed} title={reason} onClick={() => setChosen(row)}>{t('mcpImport.import')}</Button>{reason !== undefined && <span className="text-sm text-text-muted">{reason}</span>}</div> },
  ];
  return <section className="flex min-w-0 flex-col gap-3" aria-label={t('mcpImport.found')}>
    <h3 className="font-medium">{t('mcpImport.found')}</h3>
    {found.length === 0 ? <EmptyState compact title={t(right.reason === 'policy' ? 'mcpImport.policyOff' : 'mcpImport.empty')} /> : <Table label={t('mcpImport.found')} columns={columns} rows={found} rowKey={discoveryKey} />}
    {chosen !== null && <ImportMcp environment={environment} machineId={machineId} found={chosen} right={right} onClose={() => setChosen(null)}
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
          <Select label={t('mcpImport.environment')} value={environment?.id ?? NO_ENVIRONMENT} onValueChange={setSelected}
            options={[{ value: NO_ENVIRONMENT, label: t('mcpImport.chooseEnvironment'), disabled: true }, ...rows.map((row) => ({ value: row.id, label: `${row.machine?.name ?? ''} / ${row.name}` }))]} />
          {environment !== undefined && <FoundMcp key={environment.id} environment={environment} machineId={environment.machine?.id} />}
        </>}
    </div>
  </Dialog>;
}
