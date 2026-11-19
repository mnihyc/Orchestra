/** Storage's categorical state lives in the address; typed locator text never enters it. */
import type { StorageDimension, StorageKind, KeptWhy, StorageQuery } from '@/generated/api-types';

export const STORAGE_KINDS = ['deleted_place', 'environment_files', 'volume', 'mount_data', 'kept_layer', 'skill_version', 'unit', 'network', 'image', 'build', 'tool_version', 'database', 'content', 'upload', 'export', 'release', 'dump'] as const satisfies readonly StorageKind[];
export const STORAGE_WHYS = ['in_use', 'restore', 'cache', 'unknown'] as const satisfies readonly KeptWhy[];
export const STORAGE_DIMENSIONS = ['host', 'kind', 'place', 'owner', 'age', 'why'] as const satisfies readonly StorageDimension[];
const AGES = ['under_7_days', '7_to_30_days', '30_days_plus', 'unknown'] as const;
export interface StorageSearch {
  scope?: string;
  kind?: StorageKind;
  why?: KeptWhy;
  age?: string;
  host?: string;
  place?: string;
  owner?: string;
  resource_kind?: NonNullable<StorageQuery['resource_kind']>;
  resource_id?: string;
  by?: StorageDimension;
  cursor?: string;
}

export type StoragePatch = { [Key in keyof StorageSearch]?: StorageSearch[Key] | undefined };

const identifier = (value: unknown): value is string => typeof value === 'string' && /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/iu.test(value);
const oneOf = <Value extends string>(values: readonly Value[], value: unknown): Value | undefined => values.find((candidate) => candidate === value);

export function readStorageSearch(raw: Record<string, unknown>): StorageSearch {
  const result: StorageSearch = {};
  if (typeof raw.scope === 'string' && (raw.scope === 'system' || /^(?:host|node):/u.test(raw.scope) && identifier(raw.scope.split(':')[1]))) result.scope = raw.scope;
  const kind = oneOf(STORAGE_KINDS, raw.kind), why = oneOf(STORAGE_WHYS, raw.why), by = oneOf(STORAGE_DIMENSIONS, raw.by), age = oneOf(AGES, raw.age);
  if (kind !== undefined) result.kind = kind;
  if (why !== undefined) result.why = why;
  if (by !== undefined) result.by = by;
  if (age !== undefined) result.age = age;
  for (const key of ['host', 'place', 'owner'] as const) if (identifier(raw[key]) || raw[key] === 'unknown' || key === 'host' && raw[key] === 'server') result[key] = raw[key];
  if (identifier(raw.resource_id)) result.resource_id = raw.resource_id;
  const resourceKind = oneOf(['provider', 'agent', 'environment', 'host', 'machine', 'template', 'mcp_server', 'mount', 'skill', 'org', 'project', 'topic'] as const, raw.resource_kind);
  if (result.resource_id !== undefined && resourceKind !== undefined) result.resource_kind = resourceKind;
  if (result.resource_kind === undefined) delete result.resource_id;
  if (typeof raw.cursor === 'string' && raw.cursor !== '') result.cursor = raw.cursor;
  return result;
}

export function storageRequest(search: StorageSearch): StorageQuery {
  const query = { ...search };
  delete query.by;
  return query;
}

export function patchStorageSearch(search: StorageSearch, patch: StoragePatch): StorageSearch {
  return readStorageSearch({ ...search, cursor: undefined, ...patch });
}
