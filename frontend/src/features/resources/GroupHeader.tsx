/** The same compact group heading for resource lists and complete-choice pickers. */
import { ChevronDown, ChevronRight, Layers3 } from 'lucide-react';
import { useState } from 'react';
import type { PoolDisplayGroup, PoolKind } from '@/generated/api-types';
import { PoolDialog, type PoolDraft } from './pools/PoolDialog';
import { useTranslation } from 'react-i18next';
import { openAccess } from '@/features/access/AccessDialog';
import { PlaceLink } from '@/features/navigation/places';
import { MachineLink } from '@/features/resources/machines/MachineLink';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { TagChip } from '@/ui/TagChip';
import { UserChip } from '@/ui/UserChip';
import { RESOURCE_GROUP_MODES, type ResourceGroup, type ResourceGroupMode } from './groups';
import { Popover } from '@/ui/Popover';
import { Select } from '@/ui/Select';
import { ResourceSharingSummary } from './SharingSummary';

/** `picking`: the header of a group in a picker names the group (its name still leads to it) and folds it, as a quick
 * pick's separator does: making a pool of it and its home's sharing are acts of the lists, not of a choice being made. */
export function ResourceGroupHeader({ group, collapsed, onToggle, kinds = [], picking = false }: {
  group: ResourceGroup<unknown>; collapsed: boolean; onToggle: () => void; kinds?: readonly string[]; picking?: boolean;
}) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [pool, setPool] = useState<PoolDraft | null>(null);
  const draft = picking ? null : poolFromGroup(group, kinds);
  const Icon = collapsed ? ChevronRight : ChevronDown;
  const managed = !picking && group.home !== undefined && group.home.kind !== 'thread';
  return <div data-resource-group={group.id} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 py-1">
    <IconButton size="sm" label={t(collapsed ? 'resourceGroups.expand' : 'resourceGroups.collapse', { name: group.label })}
      aria-expanded={!collapsed} onClick={onToggle} icon={<Icon aria-hidden />} />
    <span className="min-w-0 max-w-full truncate font-medium" title={group.label}>
      {group.owner !== undefined ? <UserChip user={group.owner} size={24} />
        : group.machine !== undefined ? <MachineLink machine={group.machine} />
        : group.tag !== undefined ? <TagChip name={group.tag.name} color={group.tag.color} />
        : group.home !== undefined ? <PlaceLink place={group.home}>{group.label}</PlaceLink> : group.label}
    </span>
    <span className="shrink-0 text-sm text-text-muted tabular-nums">{formatNumber(group.rows.length, locale)}</span>
    {draft !== null && <Button size="sm" variant="ghost" aria-label={t('pools.makeFrom', { name: group.label })} onClick={() => setPool(draft)}>{t('pools.make')}</Button>}
    {pool !== null && <PoolDialog initial={pool} onClose={() => setPool(null)} />}
    {managed && group.home !== undefined && kinds.length > 0 && <ResourceSharingSummary home={group.home} kinds={kinds} />}
    {managed && group.home !== undefined && <Button size="sm" variant="ghost" aria-label={t('resourceGroups.sharingOf',{name:group.label})}
      onClick={() => { if (group.home !== undefined) openAccess({kind:group.home.kind,id:group.home.id,name:group.label}, {tab:'defaults',share:false}); }}>
      {t('resourceGroups.sharingPolicy')}
    </Button>}
  </div>;
}

export function ResourceGroupControl({ value, onChange }: { value: ResourceGroupMode; onChange: (value: ResourceGroupMode) => void }) {
  const { t } = useTranslation();
  return <Popover label={t('resourceGroups.label')} align="end" className="w-(--container-form) max-w-[calc(100vw-2rem)]"
    trigger={<Button size="sm" icon={<Layers3 aria-hidden />} aria-label={t('resourceGroups.label')}>
      <span className="max-sm:sr-only">{t('resourceGroups.current', {name:t(`resourceGroups.${value}`)})}</span>
    </Button>}>
    <Select size="sm" label={t('resourceGroups.label')} value={value} onValueChange={onChange}
      options={RESOURCE_GROUP_MODES.map((mode) => ({value:mode,name:t(`resourceGroups.${mode}`)}))} />
  </Popover>;
}


/** Preserve the exact display identity, including missing metadata, as a live member rule. */
function poolFromGroup(group: ResourceGroup<unknown>, kinds: readonly string[]): PoolDraft | null {
  const families = [...new Set(kinds.flatMap((kind): PoolKind[] => kind === 'provider' || kind === 'providers' ? ['provider']
    : kind === 'host' || kind === 'hosts' ? ['host'] : kind === 'environment' || kind === 'environments' ? ['environment']
      : kind === 'model' || kind === 'models' ? ['model'] : []))];
  const kind = families[0]; if (kind === undefined) return null;
  let identity: PoolDisplayGroup;
  if (group.id.startsWith('home:')) {
    if (group.home !== undefined) identity = { by: 'home', id: group.home.id };
    else if (group.id === 'home:system') identity = { by: 'home', id: null }; else return null;
  } else if (group.id.startsWith('owner:')) {
    if (group.owner !== undefined) identity = { by: 'owner', owner_id: group.owner.id, home_id: null };
    else if (group.home !== undefined) identity = { by: 'owner', owner_id: null, home_id: group.home.id };
    else if (group.id === 'owner:system') identity = { by: 'owner', owner_id: null, home_id: null }; else return null;
  } else if (group.id.startsWith('machine:')) {
    identity = { by: 'machine', id: group.id === 'machine:none' ? null : group.machine?.placement === 'managed' ? group.machine.host_id ?? group.machine.id : group.machine?.id ?? null };
  } else if (group.id.startsWith('tag:')) identity = { by: 'tag', id: group.tag?.id ?? null };
  else return null;
  return { kind, kinds: families, group: identity, name: group.label,
    ...(group.home === undefined || !['org', 'project'].includes(group.home.kind) || group.home.name === null ? {} : { home: { id: group.home.id, name: group.home.name } }) };
}
