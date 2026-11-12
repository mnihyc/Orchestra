/** The same compact group heading for resource lists and complete-choice pickers. */
import { ChevronDown, ChevronRight, Layers3 } from 'lucide-react';
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

export function ResourceGroupHeader({ group, collapsed, onToggle, kinds = [] }: {
  group: ResourceGroup<unknown>; collapsed: boolean; onToggle: () => void; kinds?: readonly string[];
}) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const Icon = collapsed ? ChevronRight : ChevronDown;
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
    {group.home !== undefined && group.home.kind !== 'thread' && kinds.length > 0 && <ResourceSharingSummary home={group.home} kinds={kinds} />}
    {group.home !== undefined && group.home.kind !== 'thread' && <Button size="sm" variant="ghost" aria-label={t('resourceGroups.sharingOf',{name:group.label})}
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
