/** Display groups over the native resource metadata; grouping never decides permission or availability. */
import type { TFunction } from 'i18next';
import type { MachineRef, NodeRef, Tag, UserRef } from '@/generated/api-types';

export const RESOURCE_GROUP_MODES = ['none', 'home', 'owner', 'machine', 'tag'] as const;
export type ResourceGroupMode = (typeof RESOURCE_GROUP_MODES)[number];

/** Sparse display input also accepts older/basic references which carry no grouping metadata. */
export interface ResourceGroupMeta {
  home?: NodeRef | null;
  home_path?: readonly NodeRef[];
  owner?: UserRef | null;
  origin_machine?: MachineRef | null;
  tags?: readonly Tag[];
  availability?: readonly { machine_id: string; available: boolean; reason: string | null }[];
}

/** Inventory metadata is outside its tagged resource; explicit nulls remain authoritative. */
export function resourceGroupMeta(input: unknown, fallback?: unknown): ResourceGroupMeta {
  const value = input !== null && typeof input === 'object' ? input as ResourceGroupMeta & { path?: readonly NodeRef[] } : {};
  const other = fallback !== null && typeof fallback === 'object' ? fallback as ResourceGroupMeta : {};
  const own = <Key extends keyof ResourceGroupMeta>(key: Key): ResourceGroupMeta[Key] => Object.hasOwn(value, key) ? value[key] : other[key];
  return { home: own('home'), owner: own('owner'), origin_machine: own('origin_machine'),
    home_path: own('home_path') ?? value.path ?? [], tags: own('tags') ?? [], availability: own('availability') ?? [] };
}

export interface ResourceGroup<Row> {
  id: string;
  label: string;
  rows: Row[];
  home?: NodeRef;
  owner?: UserRef;
  machine?: MachineRef;
  tag?: Tag;
}

export function resourceHomeLabel(meta: ResourceGroupMeta, t: TFunction): string {
  if (meta.home === undefined) return t('resourceGroups.noHome');
  if (meta.home === null) return t('resourceDetails.system');
  if (meta.home.redacted || meta.home.name === null) return t('access.place.hidden');
  const path = meta.home_path;
  return path === undefined || path.length === 0 ? meta.home.name : path.map((node) => node.redacted || node.name === null ? t('access.place.hidden') : node.name).join(' › ');
}

/** Each tag is a real membership. Counts and row selections still use the original unique page rows. */
export function groupResourceRows<Row>(rows: readonly Row[], mode: ResourceGroupMode,
  metadata: (row: Row) => ResourceGroupMeta, t: TFunction, locale: string, currentHomes: readonly string[] = []): ResourceGroup<Row>[] {
  const groups = new Map<string, ResourceGroup<Row>>();
  const add = (row: Row, group: Omit<ResourceGroup<Row>, 'rows'>) => {
    const found = groups.get(group.id);
    if (found === undefined) groups.set(group.id, { ...group, rows: [row] }); else found.rows.push(row);
  };
  for (const row of rows) {
    const meta = metadata(row);
    switch (mode) {
      case 'none': add(row, { id: 'none', label: '' }); break;
      case 'home': {
        const hidden = meta.home?.redacted === true || meta.home?.name === null;
        add(row, { id: hidden ? 'home:hidden' : `home:${meta.home?.id ?? (meta.home === null ? 'system' : 'unknown')}`,
          label: resourceHomeLabel(meta, t), ...(hidden || (meta.home === null || meta.home === undefined) ? {} : { home: meta.home }) });
        break;
      }
      case 'owner': {
        if ((meta.owner !== null && meta.owner !== undefined)) add(row, {id:`owner:${meta.owner.id}`,label:meta.owner.display_name || meta.owner.username,owner:meta.owner});
        else if (meta.owner === null && (meta.home !== null && meta.home !== undefined)) {
          const hidden = meta.home.redacted || meta.home.name === null;
          add(row, {id:hidden?'owner:home:hidden':`owner:home:${meta.home.id}`,label:resourceHomeLabel(meta,t),...(hidden?{}:{home:meta.home})});
        } else add(row, meta.owner === null && meta.home === null ? {id:'owner:system',label:t('resourceDetails.system')}
          : {id:'owner:unknown',label:t('resourceGroups.noOwner')});
        break;
      }
      case 'machine': add(row, (meta.origin_machine === null || meta.origin_machine === undefined) ? { id: 'machine:none', label: t('resourceGroups.noOrigin') }
        // The machines of a host's containers are named by their host, so the containers of one host stand in one group.
        : { id: meta.origin_machine.placement === 'managed' ? `machine:host:${meta.origin_machine.name}` : `machine:${meta.origin_machine.id}`, label: meta.origin_machine.name, machine: meta.origin_machine }); break;
      case 'tag': {
        const tags = meta.tags ?? [];
        if (tags.length === 0) add(row, { id: 'tag:none', label: t('resourceGroups.noTag') });
        else for (const tag of tags) add(row, { id: `tag:${tag.id}`, label: tag.name, tag });
        break;
      }
    }
  }
  const rank = (group: ResourceGroup<Row>) => {
    const at = mode !== 'home' || group.home === undefined ? -1 : currentHomes.indexOf(group.home.id);
    return at < 0 ? Number.POSITIVE_INFINITY : at;
  };
  return [...groups.values()].sort((one, other) => rank(one) - rank(other) || one.label.localeCompare(other.label, locale) || one.id.localeCompare(other.id));
}
