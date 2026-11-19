/** An MCP server's page: a preset's configuration, secrets and sharing; for an imported one, where it was found, the machine that keeps its definition, Import again and Forget; and where it is on.
 * Public definitions and revisioned edits use the served resource routes; imported definitions stay on their machine.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { MCP_TABS, type McpTab } from '@/features/resources/tabs';
import { Link, useNavigate } from '@tanstack/react-router';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { Plug, Pencil, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { deleteMcp, mcpQuery, reimportMcp } from '@/api/resource-tools';
import { entriesOf, shareAct } from '@/features/resources/acts';
import { ResourceDetails, ResourceTabs as Tabs, ResourceHeadingActs as HeadingActs } from '@/features/resources/DetailSections';
import { Facts } from '@/features/resources/Facts';
import { PathValue } from '@/features/resources/TechnicalValue';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { readObject } from '@/features/resources/jsonObject';
import { useResourceHeading } from '@/features/resources/trail';
import { holds } from '@/lib/permissions';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { JsonTree } from '@/ui/JsonTree';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { SettingRow } from '@/ui/SettingRow';
import { UserChip } from '@/ui/UserChip';
import { EditMcp } from './Edit';
import { connection } from './definition';
import { McpOfferings } from './Offerings';

export { MCP_TABS, type McpTab };

export function McpScreen({ id, tab, onTab }: { id: string; tab?: McpTab; onTab?: (tab: McpTab) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery(mcpQuery(id));
  const back = useResourceHeading('mcp', Plug, query.data?.name ?? t('common.loading'), query.data?.id);
  const [editing, setEditing] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [localTab, setLocalTab] = useState<McpTab>('overview');
  const reimport = useMutation({ mutationFn: () => reimportMcp(id) });
  const remove = useMutation({ mutationFn: () => deleteMcp(id), onSuccess: () => { void navigate({ to: '/resources/mcp' }); } });
  if (query.data === undefined) return query.error === null ? <SkeletonRows rows={6} /> : <ErrorState error={query.error} onRetry={() => void query.refetch()} />;
  const server = query.data;
  const entry = connection(readObject(JSON.stringify(server.spec)) ?? {});
  const target = server.environment === null ? { kind: 'mcp_server' as const, id, name: server.name } : { kind: 'environment' as const, id: server.environment.id, name: server.environment.name };
  const manage = holds(server.access, server.environment === null ? 'mcp.manage' : 'environment.manage');
  const admin = adminSpec(server, server.environment === null ? 'mcp.manage' : 'environment.manage');
  const shown = tab ?? localTab;
  const edit = manage ? { id: 'edit', label: t('resources.act.editButton'), icon: <Pencil aria-hidden />, run: () => setEditing(true) } : null;
  const menu = entriesOf([shareAct(t, target, server.access)], [manage ? { id: 'delete', label: t('resourceDetails.remove'), icon: <Trash2 aria-hidden />, danger: true, run: () => setRemoving(true) } : null]);
  const facts = [
      { name: t('resources.home.label'), value: server.home?.name ?? t('resourceDetails.system') },
      { name: t('resourceDetails.enabled'), value: <Badge tone={server.enabled ? 'accent' : 'neutral'}>{t(server.enabled ? 'resourceDetails.on' : 'resourceDetails.off')}</Badge> },
      ...(server.imported === null ? [] : [{ name: t('resourceDetails.mcp.machine'), value: <McpOfferings server={server} /> }]),
      ...(server.environment === null ? [] : [{ name: t('views.environment.title'), value: <ResourceChip subject={{ kind: 'environment', environment: server.environment }} asChild hint={false}><Link to="/resources/environments/$id" params={{ id: server.environment.id }}>{server.environment.name}</Link></ResourceChip> }]),
      { name: t('resourceDetails.mcp.form.type'), value: t(entry.transport === 'http' ? 'resourceDetails.mcp.form.http' : 'resourceDetails.mcp.form.stdio') },
      ...(entry.command === '' ? [] : [{ name: t('resourceDetails.mcp.form.command'), value: /[\\/]/u.test(entry.command) ? <PathValue path={entry.command} /> : <code>{entry.command}</code> }]),
      ...(entry.url === '' ? [] : [{ name: t('resourceDetails.mcp.form.url'), value: <span className="wrap-anywhere">{entry.url}</span> }]),
      ...(server.imported !== null ? [{ name: t('resourceDetails.layout.importedAt'), value: <RelativeTime at={server.imported.at} /> }] : [{ name: t('resourceDetails.mcp.form.authentication'), value: t(entry.authentication === 'bearer' ? 'resourceDetails.mcp.form.bearer' : entry.authentication === 'variables' ? 'resourceDetails.mcp.form.variables' : 'resourceDetails.mcp.form.none') }]),
  ];
  const overview = <>
    <Section id="mcp-overview" title={t('resourceDetails.overview')}><div id="mcp-definition"><Facts compact label={t('resourceDetails.overview')} rows={facts} /></div></Section>
    {server.imported !== null && <div className="flex flex-col items-start gap-2">
      {server.imported.stale && <InlineAlert tone="warning">{t('resourceDetails.mcp.stale')}</InlineAlert>}
      {manage && (server.offerings ?? []).some((offering) => offering.state === 'ready') && <Button admin={admin} pending={reimport.isPending} onClick={() => reimport.mutate()}>{t('resourceDetails.mcp.reimport')}</Button>}
      {reimport.error !== null && <ErrorAlert error={reimport.error} />}
    </div>}
    <ResourceDetails><Facts rows={[
      { name: t('resourceDetails.address'), value: server.address },
      ...(server.imported === null ? [] : [
        { name: t('resourceDetails.layout.source'), value: server.imported.source },
        { name: t('resourceDetails.layout.originalName'), value: server.imported.name },
        { name: t('resourceDetails.layout.installedBy'), value: <UserChip user={server.imported.by} /> },
      ]),
    ]} /><JsonTree value={server.spec} label={t('resourceDetails.mcp.form.json')} /></ResourceDetails>
  </>;
  return <Page><PageHeader adminAccess={server.access.admin_access} ownership={server} opening={{ kind: 'mcp_server', id: server.id }} back={back} title={server.name} lead={server.description} mark={<PlaceMark><Plug aria-hidden /></PlaceMark>}
    actions={<HeadingActs admin={admin} name={server.name} primary={null} edit={shown === 'settings' ? null : edit} menu={menu} />} />
    <Tabs label={t('resourceDetails.layout.tabs')} value={shown} onValueChange={onTab ?? setLocalTab} panelClassName="pt-4" tabs={[
      { value: 'overview', label: t('resourceDetails.overview'), panel: overview },
      { value: 'settings', label: t('resourceDetails.layout.settings'), panel: <ul aria-label={t('resourceDetails.layout.configuration')} className="divide-y divide-border rounded-lg border border-border">
        <SettingRow name={t('resourceDetails.layout.configuration')} value={server.imported === null ? entry.command === '' ? entry.url : /[\\/]/u.test(entry.command) ? <PathValue path={entry.command} /> : entry.command : t('resourceDetails.mcp.importedDefinition')} readOnly={!manage}
          actions={edit === null ? undefined : <Button size="sm" onClick={edit.run}>{t('resources.act.editButton')}</Button>} />
      </ul> },
      { value: 'access', label: t('agents.tabs.access'), panel: <ResourceAccess access={server.access} target={target} /> },
    ]} />
    {editing && <EditMcp server={server} onClose={() => setEditing(false)} />}
    <ConfirmDialog admin={admin} open={removing} onOpenChange={setRemoving} title={t('resourceDetails.removeTitle', { name: server.name })} confirmLabel={t('resourceDetails.remove')} danger pending={remove.isPending} error={remove.error} onConfirm={() => remove.mutate()}>{t(server.imported === null ? 'resourceDetails.mcp.removeHelp' : 'resourceDetails.mcp.forgetHelp')}</ConfirmDialog>
  </Page>;
}
