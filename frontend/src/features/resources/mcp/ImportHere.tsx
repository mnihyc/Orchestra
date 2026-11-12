/** One click imports each received origin with enable:true; individual accepted acts are never retried. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { environmentKey, environmentMcpKey, importEnvironmentMcp } from '@/api/environments';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { EnvironmentMcp, EnvironmentRef, MachineRef, McpFound } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { FoundMcp } from './Import';
import { useImportState } from './ImportState';
import { discoveryKey, validMcpName } from './importValues';
import { mcpProgram } from './words';

type EnvironmentAt = Pick<EnvironmentRef, 'id' | 'name'>;
type MachineAt = Pick<MachineRef, 'id' | 'name' | 'online'>;
interface Asked { environment: EnvironmentAt; machine: MachineAt; rows: readonly McpFound[]; reader: ReturnType<typeof libraryReader> }

function ProgramImport({ environment, machine, source, rows, right }: { environment: EnvironmentAt; machine: MachineAt; source: string; rows: readonly McpFound[]; right: EnvironmentMcp['import'] }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const cache = useQueryClient();
  const state = useImportState(environment.id, machine.id);
  const keys = useRef(new Map<string, string>());
  const [review, setReview] = useState(false);
  const needsNames = rows.some((row) => !validMcpName(row.name)) || new Set(rows.map((row) => row.name)).size !== rows.length;
  const create = useMutation({
    mutationFn: async (asked: Asked) => {
      for (const row of asked.rows) {
        if (!sameLibraryReader(asked.reader)) return;
        const identity = JSON.stringify([asked.reader, asked.environment.id, asked.machine.id, discoveryKey(row)]);
        let key = keys.current.get(identity);
        if (key === undefined) { key = newActKey(); keys.current.set(identity, key); }
        const made = await importEnvironmentMcp(asked.environment.id, { name: row.name, source: row.source, scope: row.scope, directory: row.directory, enable: true }, key);
        if (!sameLibraryReader(asked.reader)) return;
        state.acknowledge(row, made.id); keys.current.delete(identity);
      }
    },
    onSettled: (_value, _error, asked) => {
      if (!sameLibraryReader(asked.reader)) return;
      void cache.invalidateQueries({ queryKey: environmentMcpKey(asked.environment.id) });
      void cache.invalidateQueries({ queryKey: environmentKey(asked.environment.id), exact: true });
      void cache.invalidateQueries({ queryKey: ['mcp-servers'] });
    },
  });
  const reason = !right.allowed ? t(right.reason === 'policy' ? 'mcpImport.policyOff' : right.reason === 'not_owner' ? 'mcpImport.notOwner' : 'mcpImport.unavailable')
    : !machine.online ? t('mcpWords.reasons.offline') : needsNames ? t('mcpImport.chooseNames') : null;
  return <div className="flex min-w-0 flex-col gap-1">
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm">
      <span>{t('mcpImport.here', { program: mcpProgram(source), machine: machine.name, count: rows.length, number: formatNumber(rows.length, locale) })}</span>
      <Button size="sm" variant="ghost" pending={create.isPending} blocked={reason !== null} title={reason ?? undefined}
        onClick={() => create.mutate({ environment, machine, rows: [...rows], reader: libraryReader() })}>{t('mcpImport.importEnable')}</Button>
      {reason !== null && <span className="text-text-muted">{reason}</span>}
      {needsNames && right.allowed && <Button size="sm" variant="ghost" onClick={() => setReview(true)}>{t('mcpImport.review')}</Button>}
    </div>
    {create.error !== null && <ErrorAlert error={create.error} />}
    {review && <Dialog open title={t('mcpImport.title')} onOpenChange={(open) => { if (!open) setReview(false); }} size="lg" footer={<Button onClick={() => setReview(false)}>{t('common.close')}</Button>}>
      <FoundMcp environment={environment} machineId={machine.id} />
    </Dialog>}
  </div>;
}

export function ImportHere({ environment, machine, value }: { environment: EnvironmentAt; machine: MachineAt; value: EnvironmentMcp }) {
  const state = useImportState(environment.id, machine.id);
  const groups = new Map<string, McpFound[]>();
  const seen = new Set<string>();
  for (const found of value.found) {
    const key = discoveryKey(found);
    // A missing optional association is only an unlinked received origin, never global absence.
    if (found.imported !== null || state.accepted[key] !== undefined || seen.has(key)) continue;
    seen.add(key); groups.set(found.source, [...(groups.get(found.source) ?? []), found]);
  }
  return groups.size === 0 ? null : <div className="flex min-w-0 flex-col gap-2" data-mcp-import-here="">
    {[...groups].map(([source, rows]) => <ProgramImport key={`${environment.id}:${machine.id}:${source}`} environment={environment} machine={machine} source={source} rows={rows} right={value.import} />)}
  </div>;
}
