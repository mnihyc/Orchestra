/** A different native MCP definition remains a proposal until its authorized manager selects that machine's version. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { McpOfferings } from '@/features/resources/mcp/Offerings';
import type { McpServer } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';
const useOffering=vi.hoisted(()=>vi.fn());
vi.mock('@/api/resource-tools',()=>({mcpKey:(id:string)=>['mcp-server',id],selectMcpOffering:useOffering}));
const fingerprint='a'.repeat(64);
const server=(allowed:boolean,state:'found'|'ready'='found')=>({id:'server',revision:8,offerings:[{machine:{id:'host-3',name:'host-3',online:true},state,reason:'different_definition',different_definition:true,definition_fingerprint:fingerprint,can_use_definition:allowed,offered_at:state==='ready'?'2026-10-24T00:00:00Z':null,seen_at:'2026-10-24T00:00:00Z'}]}) as unknown as McpServer;
beforeEach(()=>{vi.clearAllMocks();useOffering.mockResolvedValue(server(true,'ready'));});
it('records the reviewed source and fingerprint only after the manager chooses Use',async()=>{
  const user=userEvent.setup();show(<McpOfferings server={server(true)}/>);
  expect(screen.getByText(i18n.t('mcpOffering.noMachine'))).toBeVisible();expect(screen.getByText(i18n.t('mcpOffering.noMachine')).closest('[data-status-chip]')).not.toBeNull();expect(screen.getByText(i18n.t('mcpOffering.different',{name:'host-3'}))).toBeVisible();expect(useOffering).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:i18n.t('operation.actions')}));await user.click(await screen.findByRole('menuitem',{name:i18n.t('mcpOffering.use',{name:'host-3'})}));
  await waitFor(()=>expect(useOffering).toHaveBeenCalledWith('server','host-3',{expected_revision:8,expected_fingerprint:fingerprint}));
});
it('keeps an already imported copy ready while its machine offers a different live definition',()=>{
  show(<McpOfferings server={server(true,'ready')}/>);expect(screen.queryByText(i18n.t('mcpOffering.noMachine'))).toBeNull();expect(screen.getByText(i18n.t('mcpOffering.ready'))).toBeVisible();expect(screen.getByText(i18n.t('mcpOffering.different',{name:'host-3'}))).toBeVisible();expect(useOffering).not.toHaveBeenCalled();
});
it('keeps Use disabled when server management and native import authority do not both hold',async()=>{
  const user=userEvent.setup();show(<McpOfferings server={server(false)}/>);await user.click(screen.getByRole('button',{name:i18n.t('operation.actions')}));
  const act=await screen.findByRole('menuitem',{name:new RegExp(i18n.t('mcpOffering.use',{name:'host-3'}))});expect(act).toHaveAttribute('aria-disabled','true');await user.click(act);expect(useOffering).not.toHaveBeenCalled();
});
