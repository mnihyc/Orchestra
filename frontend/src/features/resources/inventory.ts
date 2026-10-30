/** Typed resource views carried by the one authorized Everything pager. */
import type { EverythingKind, EverythingResource } from '@/generated/api-types';
import type { ResourceKind } from './trail';

export const INVENTORY_KINDS: Record<ResourceKind, EverythingKind> = {
  providers: 'provider', agents: 'agent', environments: 'environment', hosts: 'host',
  machines: 'machine', templates: 'template', mcp: 'mcp_server', mounts: 'mount', skills: 'skill',
};

export const resourceReaders = {
  providers: (row: EverythingResource) => row.kind === 'provider' ? row.value : null,
  agents: (row: EverythingResource) => row.kind === 'agent' ? row.value : null,
  environments: (row: EverythingResource) => row.kind === 'environment' ? row.value : null,
  hosts: (row: EverythingResource) => row.kind === 'host' ? row.value : null,
  machines: (row: EverythingResource) => row.kind === 'machine' ? row.value : null,
  templates: (row: EverythingResource) => row.kind === 'template' ? row.value : null,
  mcp: (row: EverythingResource) => row.kind === 'mcp_server' ? row.value : null,
  mounts: (row: EverythingResource) => row.kind === 'mount' ? row.value : null,
  skills: (row: EverythingResource) => row.kind === 'skill' ? row.value : null,
};
