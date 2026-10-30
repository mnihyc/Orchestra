/** The readable machines on which a mount has a location, linked to each machine or host's page. */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import type { Mount } from '@/generated/api-types';
import { Tooltip } from '@/ui/Tooltip';

export function MountLocationsEmpty({ hidden }: { hidden?: number | null }) {
  const { t } = useTranslation();
  if (hidden === undefined || hidden === null) return <span className="text-text-muted">{t('resourceDetails.mount.locationStatusUnknown')}</span>;
  if (hidden > 0) return <span className="text-text-muted">{t('resourceDetails.mount.locationsHidden', { count: hidden })}</span>;
  return <span className="text-text-muted">{t('resourceDetails.mount.noLocationsYet')}</span>;
}

export function MountWhere({ locations, hidden, compact = false }: Pick<Mount, 'locations'> & { hidden?: number | null; compact?: boolean }) {
  const { t } = useTranslation();
  if (compact && locations.length === 0) return <Tooltip content={<MountLocationsEmpty hidden={hidden} />}>
    <span tabIndex={0} className="block max-w-48 truncate rounded-sm text-text-muted">{t(hidden === undefined || hidden === null ? 'resourceDetails.mount.locationUnknownShort' : hidden > 0 ? 'resourceDetails.mount.locationsHiddenShort' : 'resourceDetails.mount.noLocationsShort', { count: hidden ?? 0 })}</span>
  </Tooltip>;
  if (locations.length === 0) return <MountLocationsEmpty hidden={hidden} />;
  return <ul className={compact ? 'flex max-w-48 flex-col gap-1' : 'flex flex-col gap-1'}>{locations.map(({ machine, host }) => <li key={machine.id} className="flex min-w-0 items-center gap-2">
    <Tooltip content={host?.name ?? machine.name}>
    {host === null
      ? <Link to="/resources/machines/$id" params={{ id: machine.id }} className="block min-w-0 truncate text-link hover:underline">{machine.name}</Link>
      : <Link to="/resources/hosts/$id" params={{ id: host.id }} className="block min-w-0 truncate text-link hover:underline">{host.name}</Link>}
    </Tooltip>
    {!machine.online && <span className="shrink-0 text-text-muted">{t('resourceDetails.mount.locationOffline')}</span>}
  </li>)}{hidden !== undefined && hidden !== null && hidden > 0 && <li>{compact
    ? <Tooltip content={<MountLocationsEmpty hidden={hidden} />}><span tabIndex={0} className="block truncate rounded-sm text-text-muted">{t('resourceDetails.mount.locationsHiddenShort', { count: hidden })}</span></Tooltip>
    : <MountLocationsEmpty hidden={hidden} />}</li>}</ul>;
}
