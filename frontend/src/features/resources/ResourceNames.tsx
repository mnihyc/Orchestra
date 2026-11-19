/** MCP and mount lists keep readable records and private permission-page names apart. */
import { Link, useNavigate } from '@tanstack/react-router';
import { ArrowRight, Box, Pencil, Plug, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { resourceReaders } from './inventory';
import type { McpServer, Mount } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { entriesOf, shareAct, type Act, type RowActs } from './acts';
import { ResourceList } from './ResourceList';
import { EditMcp } from './mcp/Edit';
import { EditMount } from './mounts/Edit';
import { MountWhere } from './mounts/Where';
import type { Column } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';
import { UserChip } from '@/ui/UserChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';

type NameRow = McpServer | Mount;

export function ResourceNames({ kind }: { kind: 'mcp' | 'mounts' }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [text, setText] = useState('');
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<NameRow | null>(null);
  const title = t(`screens.${kind}.title`);
  const Icon = kind === 'mcp' ? Plug : Box;
  const words = kind === 'mcp' ? 'resourceDetails.mcp.list' : 'resourceDetails.mount.list';
  const actsFor = (row: NameRow): RowActs => {
    const environment = 'spec' in row ? row.environment : null;
    const target = environment === null ? { kind: kind === 'mcp' ? 'mcp_server' as const : 'mount' as const, id: row.id, name: row.name } : { kind: 'environment' as const, id: environment.id, name: environment.name };
    const permission = environment !== null ? 'environment.manage' : kind === 'mcp' ? 'mcp.manage' : 'mount.manage';
    const open: Act = { id: 'open', label: t('resources.act.open'), icon: <ArrowRight aria-hidden />, run: () => void navigate({ to: kind === 'mcp' ? '/resources/mcp/$id' : '/resources/mounts/$id', params: { id: row.id } }) };
    const edit: Act | null = holds(row.access, permission) ? { id: 'edit', admin: adminSpec(row, permission), label: t('resources.act.editButton'), icon: <Pencil aria-hidden />, run: () => setEditing(row) } : null;
    return { busy: false, primary: edit ?? open, keys: {}, entries: entriesOf([shareAct(t, target, row.access), edit]) };
  };
  const columns: Column<NameRow>[] = [
    { id: 'name', header: t('resources.listName'), cell: (row) => <div className="flex min-w-0 flex-col gap-1">
      <ResourceChip subject={'spec' in row ? { kind: 'mcp_server', server: row } : { kind: 'mount', mount: row }} asChild hint={false}><Link to={kind === 'mcp' ? '/resources/mcp/$id' : '/resources/mounts/$id'} params={{ id: row.id }} className="focus-inset min-w-0 truncate font-medium text-text hover:underline">{row.name}</Link></ResourceChip>
      {row.description !== '' && <Tooltip content={row.description}><span tabIndex={0} className="block min-w-0 truncate rounded-sm text-sm text-text-muted">{row.description}</span></Tooltip>}
    </div> },
    { id: 'owner', header: t('resources.listOwner'), from: 'lg', cell: (row) => row.owner ? <UserChip user={row.owner} thing={'spec' in row && row.environment !== null ? { kind: 'environment', id: row.environment.id } : { kind: kind === 'mcp' ? 'mcp_server' : 'mount', id: row.id }} /> : null },
  ];
  if (kind === 'mounts') columns.splice(1, 0, { id: 'where', header: t('resourceDetails.mount.where'), from: 'sm', cell: (row) => 'locations' in row ? <MountWhere compact locations={row.locations} hidden={row.locations_hidden} /> : null });
  const add = <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>{t(kind === 'mcp' ? 'resourceDetails.mcp.add' : 'resourceDetails.mount.add')}</Button>;
  return (
    <>
      <ResourceList<NameRow>
        kind={kind}
        search={{}}
        title={title}
        icon={Icon}
        lead={t(`${words}.lead`)}
        label={title}
        readRow={kind === 'mcp' ? resourceReaders.mcp : resourceReaders.mounts}
        columns={columns}
        rowKey={(row) => row.id}
        text={text}
        onText={setText}
        searchLabel={t(`${words}.search`)}
        searchPlaceholder={t(`${words}.search`)}
        action={add}
        empty={{ icon: <Icon aria-hidden />, title: t(`${words}.none`), description: t(`${words}.noneLead`), action: add }}
        noMatch={t(`${words}.noMatch`)}
        acts={{ name: (row) => row.name, of: actsFor }}
      />
      {adding && (kind === 'mcp' ? <EditMcp onClose={() => setAdding(false)} onCreated={(row) => void navigate({ to: '/resources/mcp/$id', params: { id: row.id } })} /> : <EditMount onClose={() => setAdding(false)} onCreated={(row) => void navigate({ to: '/resources/mounts/$id', params: { id: row.id } })} />)}
      {editing !== null && ('spec' in editing ? <EditMcp server={editing} onClose={() => setEditing(null)} /> : <EditMount mount={editing} onClose={() => setEditing(null)} />)}
    </>
  );
}
