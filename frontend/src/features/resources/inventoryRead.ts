/** Mounted lists and address warmers ask for the same native inventory page. */
import { everythingQuery, type EverythingFilter, type ResourceFilter } from '@/api/everything';
import { INVENTORY_KINDS } from './inventory';
import type { ResourceKind } from './trail';

export function inventoryQuery(reader: string, kind: ResourceKind, filter: Omit<EverythingFilter, 'kinds' | 'resource' | 'owner' | 'createdFrom' | 'createdTo'>, resource: ResourceFilter) {
  return everythingQuery(reader, 'resources', { ...filter, kinds: [INVENTORY_KINDS[kind]], owner: null, createdFrom: '', createdTo: '', resource: { ...resource, rows: true } }, kind === 'mcp' ? 'mcp-servers' : kind);
}
