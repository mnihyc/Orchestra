import { ResourcePicker } from '@/ui/ResourcePicker';
/** A mount's location on each machine: path, or volume on a storage; who may add which; removal; what its data is.
 * The machine's management authority governs changes to every kind of location; the mount's managers say what the
 * data of each location is, a hint that decides nothing.
 */
import { Cleanup } from '@/features/storage/Cleanup';
import { Hint } from '@/features/help/Hint';
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';
import { machinesQuery } from '@/api/resources';
import { machineMountsQuery, mountChildrenQuery, mountKey, removeMountLocation, setMountLocation, setMountLocationDataKind } from '@/api/resource-tools';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { useRetainedRows } from '@/lib/retainedRows';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Pagination } from '@/ui/Pagination';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import type { Mount, MountDataKind, MountKeptData, MountLocation } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { StatusDot } from '@/ui/StatusDot';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';
import { Tooltip } from '@/ui/Tooltip';
import { MountLocationsEmpty } from './Where';

/** The hints in the order the screens offer them; `none` stands for a location nobody described. */
const DATA_KINDS: readonly (MountDataKind | 'none')[] = ['own', 'shared', 'synced', 'none'];

/** What a location's data is: a compact choice for the mount's managers, its name for everyone else. */
export function LocationDataKind({ mount, location, onActive }: { mount: Mount; location: MountLocation; onActive?: (active: boolean) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const set = useMutation({
    mutationFn: ({ kind }: { kind: MountDataKind | 'none'; reader: ReturnType<typeof libraryReader> }) => setMountLocationDataKind(mount.id, location.machine.id, kind === 'none' ? null : kind),
    onSuccess: (row, asked) => { if (sameLibraryReader(asked.reader)) cache.setQueryData(mountKey(row.id), row); },
  });
  useEffect(() => { onActive?.(set.isPending || set.isError); }, [onActive, set.isPending, set.isError]);
  if (!holds(mount.access, 'mount.manage')) {
    return location.data_kind === null ? null : <span>{choiceOption(t, `mountDataKind.${location.data_kind}`).name}</span>;
  }
  return <>
    <Select<MountDataKind | 'none'> size="sm" hideLabel label={t('resourceDetails.mount.dataKind')} value={location.data_kind ?? 'none'} disabled={set.isPending}
      onValueChange={(kind) => set.mutate({ kind, reader: libraryReader() })} options={DATA_KINDS.map((value) => ({ value, ...choiceOption(t, `mountDataKind.${value}`) }))} />
    {set.error !== null && <ErrorAlert error={set.error} />}
  </>;
}

function useMountChildren<Kind extends 'locations' | 'kept' | 'lost_locations'>(mount: Mount, collection: Kind) {
  const search = useCollectionSearch('mount-children:' + mount.id + ':' + collection);
  const source = mount[collection];
  const version = useMemo(() => {
    if (collection === 'locations') return JSON.stringify(mount.locations.map((row) => [row.machine.id, row.machine.name, row.host?.name, row.source, row.storage, row.data_kind]));
    if (collection === 'kept') return JSON.stringify(mount.kept.map((row) => [row.id, row.machine.name, row.host?.name, row.source, row.storage]));
    return JSON.stringify(mount.lost_locations?.map((row) => [row.id, row.machine_name, row.host_name, row.storage]));
  }, [mount, collection]);
  const paged = (source?.length ?? 0) > 50 || search.q !== '';
  const pages = useInfiniteQuery({ ...mountChildrenQuery(mount.id, collection, search.q, version), enabled: paged });
  const rows = useMemo(() => paged ? pages.data?.pages.flatMap((page) => page.items) ?? [] : (source ?? []) as NonNullable<Mount[Kind]>[number][], [paged, pages.data, source]);
  return { search, paged, pages, rows };
}
function ChildMore({ list }: { list: { paged: boolean; pages: { hasNextPage: boolean; isFetchingNextPage: boolean; isFetchNextPageError: boolean; error: unknown; fetchNextPage: () => Promise<unknown> } } }) {
  return <Pagination hasMore={list.paged && list.pages.hasNextPage} loading={list.pages.isFetchingNextPage} error={list.paged && list.pages.isFetchNextPageError ? list.pages.error : null} onMore={() => void list.pages.fetchNextPage()} />;
}

/** Data a copy displaced, attached nowhere until it is purged: a volume goes with its data once the mount's name is
 * typed, a directory is only forgotten. */
function Kept({ mount }: { mount: Mount }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [purging, setPurging] = useState<MountKeptData | null>(null);
  const list = useMountChildren(mount, 'kept');
  if (mount.kept.length === 0 && purging === null && list.search.text === '') return null;
  return <><div className="flex flex-col gap-2" data-mount-kept>
    <h4 className="text-sm font-medium text-text">{t('resourceDetails.mount.kept')}</h4>
    <CollectionSearch name={t('resourceDetails.mount.kept')} {...list.search} />
    {list.paged && list.pages.isPending ? <SkeletonRows read={list.pages} rows={3} /> : list.paged && list.pages.isError && !list.pages.isFetchNextPageError ? <ErrorState compact error={list.pages.error} onRetry={() => void list.pages.refetch()} /> : <ScrollRegion kind="table" label={t('resourceDetails.mount.kept')} className="max-h-96 overflow-y-auto"><ul className="flex flex-col divide-y divide-border">{list.rows.map((data) => <li key={data.id} className="flex flex-wrap items-center gap-2 py-2">
      <span>{data.host?.name ?? data.machine.name}</span>
      <span>{t(data.kind === 'volume' ? 'resourceDetails.mount.volume' : 'resourceDetails.mount.path')}</span>
      <code>{data.source ?? t('resourceDetails.mount.sourceHidden')}</code>
      {data.file_count !== null && data.total_bytes !== null && <span className="text-sm text-text-muted">{t('mountPlan.copy.size', { count: data.file_count, formattedCount: formatNumber(data.file_count, locale), size: formatBytes(data.total_bytes, locale) })}</span>}
      <UserChip user={data.kept_by} />
      {data.can_purge && <Button size="sm" variant="danger-ghost" onClick={() => setPurging(data)}>{t('resourceDetails.mount.purge')}</Button>}
    </li>)}</ul></ScrollRegion>}
    <ChildMore list={list} />
  </div>
    {purging !== null && <Cleanup request={{scope:'me',resource:{kind:'mount',id:mount.id},items:[`mount:${purging.id}`]}} onClose={() => setPurging(null)} />}
  </>;
}

function AddLocation({ mount, onClose }: { mount: Mount; onClose: () => void }) {
  const { t } = useTranslation();
  const machines = useQuery(machinesQuery);
  const [machine, setMachine] = useState('');
  const [kind, setKind] = useState<'path' | 'volume'>('path');
  const [source, setSource] = useState('');
  const [storage, setStorage] = useState('default');
  const offered = useQuery({ ...machineMountsQuery(machine), enabled: machine !== '' });
  const choice = offered.isError ? undefined : offered.data?.available.find((row) => row.mount.id === mount.id);
  const options = (machines.isError ? [] : machines.data ?? []).filter((row) =>
    (holds(row.access, 'machine.manage') || holds(row.access, 'host.manage')) && !mount.locations.some((location) => location.machine.id === row.id)
  ).map((row) => ({ value: row.id, resource:{kind:'machine',id:row.id,data:row}, ...choiceOption(t, 'dynamic.resource', { name: row.host?.name ?? row.name }) }));
  const canVolume = choice !== undefined && (choice.default_storage || choice.storages.length > 0);
  const valid = options.some((option) => option.value === machine) && choice !== undefined && (kind === 'path'
    ? choice.path && source.trim() !== ''
    : canVolume && (storage === 'default' ? choice.default_storage : choice.storages.includes(storage.slice(6))));
  const save = useMutation({ mutationFn: () => {
    if (!valid) throw new Error(t('resourceDetails.mount.locationUnavailable'));
    return setMountLocation(mount.id, machine, kind === 'path' ? { kind, source }
      : { kind, ...(storage === 'default' ? {} : { storage: storage.slice(6) }) });
  }, onSuccess: onClose });
  return <Dialog open onOpenChange={(open) => { if (!open && !save.isPending) onClose(); }} title={t('resourceDetails.mount.addLocation')} dismissible={!save.isPending} footer={<><Button disabled={save.isPending} onClick={onClose}>{t('common.cancel')}</Button><Button variant="primary" pending={save.isPending} disabled={!valid} onClick={() => save.mutate()}>{t('resources.edit.save')}</Button></>}>
    <div className="flex flex-col gap-3">
      <ResourcePicker pickerKey="mount-location-machine" label={t('resourceDetails.mount.machine')} value={machine} disabled={save.isPending} onValueChange={(value) => { setMachine(value); setKind('path'); setSource(''); setStorage('default'); }} options={options} />
      {choice !== undefined && <><Select<'path' | 'volume'> label={t('resourceDetails.mount.locationKind')} value={kind} disabled={save.isPending} onValueChange={(value) => { setKind(value); setStorage(choice.default_storage ? 'default' : `named:${choice.storages[0] ?? ''}`); }}
        options={[...(choice.path ? [{ value: 'path' as const, ...choiceOption(t, 'mountLocation.path') }] : []), ...(canVolume ? [{ value: 'volume' as const, ...choiceOption(t, 'mountLocation.volume') }] : [])]} />
        {kind === 'path' ? <Input label={t('resourceDetails.mount.source')} value={source} disabled={save.isPending} onChange={(event) => setSource(event.target.value)} /> : <Select label={t('resourceDetails.mount.storage')} value={storage} disabled={save.isPending} onValueChange={setStorage}
          options={[...(choice.default_storage ? [{ value: 'default', ...choiceOption(t, 'mountLocation.default') }] : []), ...choice.storages.map((name) => ({ value: `named:${name}`, ...choiceOption(t, 'dynamic.resource', { name }) }))]} />}</>}
      {machine !== '' && offered.isSuccess && choice === undefined && <p>{t('resourceDetails.mount.locationUnavailable')}</p>}
      {machines.error !== null && <ErrorAlert error={machines.error} />}{offered.error !== null && <ErrorAlert error={offered.error} />}{save.error !== null && <ErrorAlert error={save.error} />}
    </div>
  </Dialog>;
}

interface LocationEntry { id: string; location: MountLocation }
function LocationRow({ entry, mount, visible, keep, onRemove }: { entry: LocationEntry; mount: Mount; visible: boolean; keep: (entry: LocationEntry, active: boolean) => void; onRemove: (location: MountLocation) => void }) {
  const { t } = useTranslation();
  const location = entry.location;
  const active = useCallback((active: boolean) => keep(entry, active), [keep, entry]);
  return <li hidden={!visible} className="flex flex-col items-start gap-2 py-2">
        <div className="flex flex-wrap items-center gap-2">
          {location.host === null
            ? <Link to="/resources/machines/$id" params={{ id: location.machine.id }} className="text-link hover:underline">{location.machine.name}</Link>
            : <Link to="/resources/hosts/$id" params={{ id: location.host.id }} className="text-link hover:underline">{location.host.name}</Link>}
          <span>{t(location.kind === 'volume' ? 'resourceDetails.mount.volume' : 'resourceDetails.mount.path')}</span>
          <code>{location.source ?? t('resourceDetails.mount.sourceHidden')}</code>
          {location.storage !== null && <span>{location.storage}</span>}<LocationDataKind mount={mount} location={location} onActive={active} /><UserChip user={location.created_by} />
          {location.can_remove && <Button size="sm" variant="danger-ghost" onClick={() => onRemove(location)}>{t('resourceDetails.remove')}</Button>}
        </div>
        {!location.machine.online && <p className="text-sm text-text-muted">{t('resourceDetails.mount.locationOfflineHelp')}</p>}
  </li>;
}
export function Locations({ mount }: { mount: Mount }) {
  const { t } = useTranslation();
  const list = useMountChildren(mount, 'locations');
  const entries = useMemo(() => list.rows.map((location) => ({ id: location.machine.id, location })), [list.rows]);
  const held = useRetainedRows(entries);
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<MountLocation | null>(null);
  const [confirm, setConfirm] = useState('');
  const remove = useMutation({ mutationFn: async () => {
    if (removing === null) return;
    await removeMountLocation(mount.id, removing.machine.id, removing.kind === 'volume' ? confirm : undefined);
  }, onSuccess: () => { setRemoving(null); } });
  return <Section id="mount-locations" title={t('resourceDetails.mount.locations')} action={<><Hint popover id="loss-mounts" />{holds(mount.access, 'mount.use') && <Button onClick={() => setAdding(true)}>{t('resourceDetails.mount.addLocation')}</Button>}</>}>
    <CollectionSearch name={t('resourceDetails.mount.locations')} {...list.search} />
    {list.paged && list.pages.isPending ? <SkeletonRows read={list.pages} rows={3} /> : list.paged && list.pages.isError && !list.pages.isFetchNextPageError ? <ErrorState compact error={list.pages.error} onRetry={() => void list.pages.refetch()} /> : list.rows.length === 0 ? <EmptyState compact title={<MountLocationsEmpty hidden={mount.locations_hidden} />} /> : null}
    <ScrollRegion kind="table" label={t('resourceDetails.mount.locations')} className="max-h-96 overflow-y-auto"><ul className="flex flex-col divide-y divide-border">
      {held.rows.map((entry) => <LocationRow key={entry.id} entry={entry} mount={mount} visible={held.visible.has(entry.id)} keep={held.keep} onRemove={(location) => { remove.reset(); setConfirm(''); setRemoving(location); }} />)}
    </ul></ScrollRegion>
    <ChildMore list={list} />
    {(mount.locations_hidden ?? 0) > 0 && <p><MountLocationsEmpty hidden={mount.locations_hidden} /></p>}
    <LostLocations mount={mount} />
    <Kept mount={mount} />
    {adding && <AddLocation mount={mount} onClose={() => setAdding(false)} />}
    <Dialog open={removing !== null} onOpenChange={(open) => { if (!open && !remove.isPending) setRemoving(null); }} title={t('resourceDetails.mount.removeLocation')} dismissible={!remove.isPending} footer={<><Button data-autofocus disabled={remove.isPending} onClick={() => setRemoving(null)}>{t('common.cancel')}</Button><Button variant="danger" pending={remove.isPending} disabled={removing?.kind === 'volume' && confirm !== mount.name} onClick={() => remove.mutate()}>{t('resourceDetails.remove')}</Button></>}>
      <p>{t(removing?.kind === 'volume' ? 'resourceDetails.mount.volumeRemoval' : 'resourceDetails.mount.pathRemoval', { name: mount.name, machine: removing?.machine.name ?? '' })}</p>
      {removing?.kind === 'volume' && <Input label={t('resourceDetails.mount.confirmName')} value={confirm} onChange={(event) => setConfirm(event.target.value)} />}
      {remove.error !== null && <ErrorAlert error={remove.error} />}
    </Dialog>
  </Section>;
}

function LostLocations({ mount }: { mount: Mount }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const list = useMountChildren(mount, 'lost_locations');
  if ((mount.lost_locations ?? []).length === 0 && list.search.text === '') return null;
  return <div className="flex min-w-0 flex-col gap-2">
    <CollectionSearch name={t('mountLoss.locations')} {...list.search} />
    {list.paged && list.pages.isPending ? <SkeletonRows read={list.pages} rows={3} /> : list.paged && list.pages.isError && !list.pages.isFetchNextPageError ? <ErrorState compact error={list.pages.error} onRetry={() => void list.pages.refetch()} /> : <ScrollRegion kind="table" label={t('mountLoss.locations')} className="max-h-96 overflow-y-auto"><ul className="flex flex-col divide-y divide-border" aria-label={t('mountLoss.locations')}>{list.rows.map((location) => <li key={location.id} className="flex min-w-0 flex-wrap items-center gap-2 py-2"><Tooltip content={location.host_name ?? location.machine_name}><span tabIndex={0} className="min-w-0 max-w-full truncate">{location.host_name ?? location.machine_name}</span></Tooltip><StatusDot tone="warning" label={t('loss.status')} showLabel /><span className="text-sm text-text-muted">{location.total_bytes === null ? '–' : formatBytes(location.total_bytes, locale)}</span><RelativeTime at={location.lost_at} /></li>)}</ul></ScrollRegion>}
    <ChildMore list={list} />
  </div>;
}
