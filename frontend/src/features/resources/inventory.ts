/** Typed resource views carried by the one authorized Everything pager. */
import type { EverythingKind, ResourceListRow } from '@/generated/api-types';
import type { ResourceKind } from './trail';

export const INVENTORY_KINDS: Record<ResourceKind, EverythingKind> = {
  providers: 'provider', agents: 'agent', environments: 'environment', hosts: 'host',
  machines: 'machine', templates: 'template', mcp: 'mcp_server', mounts: 'mount', skills: 'skill',
};

export const resourceReaders = {
  providers: (row: ResourceListRow) => row.kind === 'provider' ? row.value : null,
  agents: (row: ResourceListRow) => row.kind === 'agent' ? row.value : null,
  environments: (row: ResourceListRow) => row.kind === 'environment' ? row.value : null,
  hosts: (row: ResourceListRow) => row.kind === 'host' ? row.value : null,
  machines: (row: ResourceListRow) => row.kind === 'machine' ? row.value : null,
  templates: (row: ResourceListRow) => row.kind === 'template' ? row.value : null,
  mcp: (row: ResourceListRow) => row.kind === 'mcp_server' ? row.value : null,
  mounts: (row: ResourceListRow) => row.kind === 'mount' ? row.value : null,
  skills: (row: ResourceListRow) => row.kind === 'skill' ? row.value : null,
};
