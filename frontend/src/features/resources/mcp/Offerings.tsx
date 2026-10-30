/** Retained native MCP definitions list their actual machines and require a manager's act for a different definition. */
import { Hint } from '@/features/help/Hint';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { MoreHorizontal } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { mcpKey, selectMcpOffering } from '@/api/resource-tools';
import type { McpServer } from '@/generated/api-types';
import { ContextMenu } from '@/ui/ContextMenu';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import { StatusDot } from '@/ui/StatusDot';
import { Tooltip } from '@/ui/Tooltip';

export function McpOfferings({ server }: { server: McpServer }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const select = useMutation({ mutationFn: (machine: string) => {
    const offering = server.offerings?.find((row) => row.machine.id === machine);
    if (offering === undefined) throw new Error('The offering changed');
    return selectMcpOffering(server.id, machine, { expected_revision: server.revision, expected_fingerprint: offering.definition_fingerprint ?? null });
  }, onSuccess: (accepted) => { client.setQueryData(mcpKey(server.id), accepted); void client.invalidateQueries({ queryKey: ['mcp-servers'] }); } });
  const offers = server.offerings ?? [];
  return <div className="flex min-w-0 flex-col gap-2">
    <div className="flex flex-wrap items-center gap-1">{!offers.some((row) => row.state === 'ready') && <StatusDot tone="warning" label={t('mcpOffering.noMachine')} showLabel />}<Hint popover id="loss-native" /></div>
    <ul className="flex min-w-0 flex-col gap-2">{offers.map((offering) => {
      const entries: MenuEntry[] = offering.state === 'found'||offering.different_definition ? [{ id: offering.machine.id, label: t('mcpOffering.use', { name: offering.machine.name }), disabled: !offering.can_use_definition ? t('mcpOffering.noImportPermission') : select.isPending ? t('common.loading') : false, onSelect: () => select.mutate(offering.machine.id) }] : [];
      return <li key={offering.machine.id}><ContextMenu label={offering.machine.name} entries={entries}><div className="flex min-w-0 flex-wrap items-center gap-2">
        <Tooltip content={offering.machine.name}><a href={`/resources/machines/${offering.machine.id}`} className="min-w-0 truncate font-medium text-text hover:underline">{offering.machine.name}</a></Tooltip>
        <StatusDot tone={offering.state === 'ready' ? 'success' : offering.state === 'pending' ? 'info' : 'warning'} label={t(offering.state==='found'&&offering.reason==='different_definition'?'mcpOffering.different':`mcpOffering.${offering.state}`,{name:offering.machine.name})} showLabel />
        {offering.state==='ready'&&offering.different_definition&&<span className="text-sm text-warning">{t(offering.reason==='different_definition'?'mcpOffering.different':'mcpOffering.unverified',{name:offering.machine.name})}</span>}
        {offering.offered_at !== null && offering.state === 'ready' && <RelativeTime at={offering.offered_at} />}
        {entries.length > 0 && <Menu label={offering.machine.name} entries={entries} trigger={<IconButton label={t('operation.actions')} icon={<MoreHorizontal aria-hidden />} />} />}
      </div></ContextMenu></li>;
    })}</ul>
    {select.error !== null && <ErrorAlert error={select.error} />}
  </div>;
}
