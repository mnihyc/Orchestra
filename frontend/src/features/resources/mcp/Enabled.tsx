/** Named MCP use writes the existing own boolean map and displays native explicit-giving facts. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ReceiptAcknowledged } from '@/api/client';
import { environmentKey, environmentMcpKey } from '@/api/environments';
import { libraryKey, libraryQuery, libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { mcpKey, mcpServersQuery } from '@/api/resource-tools';
import { putSetting, targetSettingsKey, targetSettingsQuery, type SettingsTarget, type SettingWrite } from '@/api/settings';
import { KnownNodeChip } from '@/features/people/NamedChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { resourceSubjectOf } from '@/features/resource-facts/facts';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import type { Environment, McpServer, ResolvedSetting, VisibleDependent } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { DetailCard } from '@/ui/DetailCard';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { MCP_ENABLED, McpEnabledSwitch } from './EnabledSwitch';

interface Asked { target: SettingsTarget; server: string; write: SettingWrite; reader: ReturnType<typeof libraryReader> }
const flag = (value: unknown, id: string): unknown => typeof value === 'object' && value !== null && id in value ? (value as Record<string, unknown>)[id] : undefined;

export function McpSwitchAt({ target, server, blocked }: { target: SettingsTarget; server: Pick<McpServer, 'id' | 'name' | 'enabled'>; blocked?: string }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const rows = useQuery(targetSettingsQuery(target));
  const change = useMutation({ mutationFn: async (asked: Asked) => {
    try { return { row: await putSetting(asked.target, MCP_ENABLED, asked.write) }; }
    catch (error) { if (error instanceof ReceiptAcknowledged) return { row: null }; throw error; }
  }, onSuccess: ({ row }, asked) => {
    if (!sameLibraryReader(asked.reader)) return;
    if (row !== null) cache.setQueryData<ResolvedSetting[]>(targetSettingsKey(asked.target), (before) => before?.map((entry) => entry.key === MCP_ENABLED ? row : entry));
    void cache.invalidateQueries({ queryKey: targetSettingsKey(asked.target), exact: true });
    void cache.invalidateQueries({ queryKey: mcpKey(asked.server), exact: true });
    void cache.invalidateQueries({ queryKey: libraryKey([{ object: { kind: 'mcp_server', id: asked.server }, version_id: null }]), exact: true });
    if (asked.target.kind === 'environment') {
      void cache.invalidateQueries({ queryKey: environmentMcpKey(asked.target.id) });
      void cache.invalidateQueries({ queryKey: environmentKey(asked.target.id), exact: true });
    }
  }, onError: (_error, asked) => { if (sameLibraryReader(asked.reader)) void cache.invalidateQueries({ queryKey: targetSettingsKey(asked.target), exact: true }); } });
  if (rows.isError) return <ErrorState compact error={rows.error} onRetry={() => void rows.refetch()} />;
  if (rows.data === undefined) return <SkeletonRows rows={1} />;
  const setting = rows.data.find((row) => row.key === MCP_ENABLED);
  if (setting === undefined) return <span className="text-sm text-text-muted">{t('mcpSwitch.unavailable')}</span>;
  const asked = change.variables;
  const thisAct = asked?.server === server.id && JSON.stringify(asked.target) === JSON.stringify(target) && sameLibraryReader(asked.reader);
  const readback = asked !== undefined && thisAct && change.data?.row === null && flag(setting.own?.value, server.id) !== flag(asked.write.value, server.id);
  return <McpEnabledSwitch server={server} setting={setting} blocked={blocked} pending={change.isPending || readback}
    pendingLabel={readback ? t('mcpSwitch.accepted') : undefined} error={thisAct ? change.error : null}
    onApply={(write) => change.mutate({ target: { ...target }, server: server.id, write, reader: libraryReader() })} />;
}

export function EnvironmentMcpChoices({ environment }: { environment: Pick<Environment, 'id' | 'machine'> }) {
  const { t } = useTranslation();
  const available = useQuery(mcpServersQuery);
  if (available.isError) return <ErrorState compact error={available.error} onRetry={() => void available.refetch()} />;
  if (available.data === undefined) return <SkeletonRows rows={2} />;
  const servers = available.data.filter((server) => holds(server.access, 'mcp.use'));
  return <section className="flex min-w-0 flex-col gap-3" aria-label={t('mcpSwitch.available')}>
    <h3 className="font-medium">{t('mcpSwitch.available')}</h3>
    {servers.length === 0 ? <EmptyState compact title={t('mcpSwitch.noneAvailable')} /> : <ul className="flex min-w-0 flex-col gap-3">{servers.map((server) => {
      const blocked = server.imported !== null && (server.machine === null || server.machine.id !== environment.machine?.id)
        ? t(server.machine === null || environment.machine === null ? 'mcpWords.reasons.noMachine' : 'mcpWords.reasons.otherMachine') : undefined;
      return <li key={server.id} className="flex min-w-0 flex-col gap-1"><ResourceChip subject={{ kind: 'mcp_server', server }} hint={false} />
        <McpSwitchAt target={{ kind: 'environment', id: environment.id }} server={server} blocked={blocked} /></li>;
    })}</ul>}
  </section>;
}

function ScopeName({ row }: { row: VisibleDependent }) {
  const object = row.object;
  if (object === null) return <span className="text-sm text-text-muted">{row.name}</span>;
  if (object.kind === 'org' || object.kind === 'project' || object.kind === 'topic' || object.kind === 'thread') return <KnownNodeChip node={{ kind: object.kind, id: object.id, name: row.name, redacted: false }} />;
  const subject = resourceSubjectOf(object, row.name);
  return subject === null ? <span className="text-sm text-text-muted">{row.name}</span> : <ResourceChip subject={subject} hint={false} />;
}

export function McpEnabledIn({ server }: { server: McpServer }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const facts = useQuery(libraryQuery([{ object: { kind: 'mcp_server', id: server.id }, version_id: null }]));
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const observation = facts.data?.find((row) => row.target.object.id === server.id)?.facts;
  const group = observation?.dependents.find((entry) => entry.relation === 'explicit_giving');
  const visible = group?.visible ?? [];
  const label = observation?.dependents_complete === true
    ? t('mcpSwitch.placeCount', { count: group?.count ?? 0, number: formatNumber(group?.count ?? 0, locale) })
    : t('mcpSwitch.observed', { number: formatNumber(group?.count ?? 0, locale) });
  const mayUse = holds(server.access, server.environment === null ? 'mcp.use' : 'environment.use');
  return <section className="flex min-w-0 flex-col gap-3" aria-label={t('mcpSwitch.where')}>
    <div className="flex min-w-0 flex-wrap items-center gap-1"><h3 className="font-medium">{t('mcpSwitch.where')}</h3>
      {facts.isPending ? <SkeletonRows rows={1} /> : facts.isError || observation === undefined ? <span>–</span>
        : visible.length > 0 ? <DetailCard label={label} content={() => <div className="flex min-w-0 flex-col gap-2 p-3"><ul className="flex flex-col gap-1">{visible.map((row, index) => <li key={`${row.object?.id ?? ''}:${index}`}><ScopeName row={row} /></li>)}</ul>{(group?.hidden ?? 0) > 0 && <span className="text-sm text-text-muted">{t('library.more', { number: formatNumber(group?.hidden ?? 0, locale) })}</span>}</div>}>
          <button type="button" className="focus-inset min-w-0 rounded-sm text-sm text-text-muted underline decoration-dotted underline-offset-2">{visible[0]?.name}{visible.length > 1 ? ` · ${label}` : ''}</button>
        </DetailCard> : group !== undefined && group.count > 0 ? <span>{label}</span> : <span>{observation.dependents_complete ? t('mcpSwitch.nowhere') : '–'}</span>}
    </div>
    {facts.isError && <ErrorAlert error={facts.error} action={<Button size="sm" onClick={() => void facts.refetch()}>{t('common.tryAgain')}</Button>} />}
    {mayUse ? server.environment !== null ? <McpSwitchAt target={{ kind: 'environment', id: server.environment.id }} server={server} />
      : <><PlacePicker label={t('mcpSwitch.place')} value={place} onChange={setPlace} includeThreads />
        {place !== null && <McpSwitchAt key={place.id} target={{ kind: 'node', id: place.id }} server={server} />}</>
      : <span className="text-sm text-text-muted">{t('mcpSwitch.needsUse')}</span>}
  </section>;
}
