import { CompoundPickerScope } from '@/features/resources/PickerContext';
import { ResourcePickerLookup } from '@/ui/ResourcePicker';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Pagination } from '@/ui/Pagination';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { useCollectionSearch } from '@/lib/collectionSearch';
/** Mount locations on one machine, with reported sizes, readable attachments and the ordinary guarded add/remove acts. */
import { useInfiniteQuery, useMutation } from '@tanstack/react-query';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { MoreHorizontal, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from '@tanstack/react-router';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';
import { machineMountsPageQuery, removeMountLocation, setMountLocation } from '@/api/resource-tools';
import type { MachineMount, MachineMountChoice } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { CopyMountDataLoader } from './Copy';

function AddHere({ machine, onClose }: { machine: string; onClose: () => void }) {
  const { t } = useTranslation();
  const search = useCollectionSearch('mount-additions:' + machine);
  const query = useInfiniteQuery(machineMountsPageQuery(machine, 'available', search.q));
  const choices = query.data?.pages.flatMap((page) => page.available) ?? [];
  const [choice, setChoice] = useState<MachineMountChoice>();
  const [kind, setKind] = useState<'path' | 'volume'>('path');
  const [source, setSource] = useState('');
  const [storage, setStorage] = useState('default');
  const canVolume = choice !== undefined && (choice.default_storage || choice.storages.length > 0);
  const valid = choice !== undefined && (kind === 'path'
    ? choice.path && source.trim() !== ''
    : canVolume && (storage === 'default' ? choice.default_storage : choice.storages.includes(storage.slice(6))));
  const save = useMutation({ mutationFn: () => {
    if (!valid) throw new Error(t('resourceDetails.mount.here.choose'));
    return setMountLocation(choice.mount.id, machine, kind === 'path'
      ? { kind, source }
      : { kind, ...(storage === 'default' ? {} : { storage: storage.slice(6) }) });
  }, onSuccess: onClose });
  return <Dialog open onOpenChange={(open) => { if (!open && !save.isPending) onClose(); }} title={t('resourceDetails.mount.here.add')} dismissible={!save.isPending} footer={<>
    <Button disabled={save.isPending} onClick={onClose}>{t('common.cancel')}</Button>
    <Button variant="primary" pending={save.isPending} disabled={!valid} onClick={() => save.mutate()}>{t('resources.edit.save')}</Button>
  </>}>
    <div className="flex flex-col gap-3">
      <ResourcePickerLookup pickerKey="machine-mount-addition" label={t('resourceDetails.mount.here.mount')} disabled={save.isPending}
        query={search.text} onQueryChange={search.onText} loading={query.isPending} displayValue={choice?.mount.name} placeholder={t('resourceDetails.mount.here.choose')}
        groups={[{ id: 'available', label: '', options: choices.map((choice) => ({ value: choice.mount.id, label: choice.mount.name, text: choice.mount.name, ...resourceChoice({ kind: 'mount', mount: choice.mount }), description: choice.mount.address })) }]}
        empty={query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : t('resourceDetails.mount.here.noChoices')}
        footer={<Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} />}
        onSelect={(option) => {
          const next = choices.find((row) => row.mount.id === option.value);
          setChoice(next); setKind(next?.path === true ? 'path' : 'volume'); setSource('');
          setStorage(next?.default_storage === true ? 'default' : `named:${next?.storages[0] ?? ''}`);
        }} />
      {choice !== undefined && <Select<'path' | 'volume'> label={t('resourceDetails.mount.locationKind')} value={kind} disabled={save.isPending} onValueChange={setKind}
        options={[...(choice.path ? [{ value: 'path' as const, ...choiceOption(t, 'mountLocation.path') }] : []), ...(canVolume ? [{ value: 'volume' as const, ...choiceOption(t, 'mountLocation.volume') }] : [])]} />}
      {choice !== undefined && (kind === 'path'
        ? <Input label={t('resourceDetails.mount.source')} value={source} disabled={save.isPending} onChange={(event) => setSource(event.target.value)} />
        : <ChoiceLocator label={t('resourceDetails.mount.storage')} value={storage} disabled={save.isPending} onValueChange={setStorage}
          options={[...(choice.default_storage ? [{ value: 'default', ...choiceOption(t, 'mountLocation.default') }] : []), ...choice.storages.map((name) => ({ value: `named:${name}`, ...choiceOption(t, 'dynamic.resource', { name }) }))]} />)}
      {save.error !== null && <ErrorAlert error={save.error} />}
    </div>
  </Dialog>;
}

function Here({ machine, name }: { machine: string; name: string }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const search = useCollectionSearch('mount-locations:' + machine);
  const query = useInfiniteQuery(machineMountsPageQuery(machine, 'locations', search.q));
  const locations = query.data?.pages.flatMap((page) => page.locations) ?? [];
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<MachineMount | null>(null);
  const [copying, setCopying] = useState<MachineMount | null>(null);
  const [confirm, setConfirm] = useState('');
  const remove = useMutation({ mutationFn: async () => {
    if (removing === null) return;
    await removeMountLocation(removing.mount.id, machine, removing.kind === 'volume' ? confirm : undefined);
  }, onSuccess: () => setRemoving(null) });
  const data = query.isError && !query.isFetchNextPageError ? undefined : query.data?.pages.at(-1);
  const entries = (row: MachineMount): MenuEntry[] => [
    { id: 'copy-data', label: t('mountCopy.copy'), disabled: row.can_remove && row.source !== null ? false : t('mountCopy.noSource'), onSelect: () => setCopying(row) },
    { id: 'remove', label: t('resourceDetails.mount.here.remove', { name: row.mount.name }), danger: true, disabled: row.can_remove ? false : t('resources.act.manageRequired'), onSelect: () => { setRemoving(row); setConfirm(''); remove.reset(); } },
  ];
  const columns: Column<MachineMount>[] = [
    { id: 'mount', header: t('resourceDetails.mount.here.mount'), cell: (row) => <ResourceChip subject={{ kind: 'mount', mount: row.mount }} asChild hint={false}><Link to="/resources/mounts/$id" params={{ id: row.mount.id }}>{row.mount.name}</Link></ResourceChip> },
    { id: 'location', header: t('resourceDetails.mount.locations'), cell: (row) => <div className="flex min-w-0 flex-col gap-1">
      <span>{t(row.kind === 'volume' ? 'resourceDetails.mount.volume' : 'resourceDetails.mount.path')}</span>
      <code className="wrap-anywhere">{row.source ?? t('resourceDetails.mount.sourceHidden')}</code>
      {row.kind === 'volume' && <span className="text-sm text-text-muted">{row.storage ?? t('resourceDetails.mount.defaultStorage')}</span>}
      {row.data_kind !== null && <span className="text-sm text-text-muted">{choiceOption(t, `mountDataKind.${row.data_kind}`).name}</span>}
    </div> },
    { id: 'size', header: t('resourceDetails.mount.here.size'), cell: (row) => row.size_bytes === null ? t('resourceDetails.mount.here.sizeUnknown') : formatBytes(row.size_bytes, locale) },
    { id: 'environments', header: t('resourceDetails.mount.here.environments'), cell: (row) => <AttachedEnvironments machine={machine} row={row} /> },
    { id: 'actions', header: t('operation.actions'), hideHeader: true, cell: (row) => <Menu label={row.mount.name} entries={entries(row)} trigger={<IconButton label={t('operation.actions')} icon={<MoreHorizontal aria-hidden />} />} /> },
  ];
  return <CompoundPickerScope data={data} screen="machine-mount-location">{<Section id={`mounts-here-${machine}`} title={t('resourceDetails.mount.here.title')} action={<Button variant="primary" icon={<Plus aria-hidden />} disabled={data === undefined || data.available.length === 0} onClick={() => setAdding(true)}>{t('resourceDetails.mount.here.add')}</Button>}>
    <CollectionSearch name={t('resourceDetails.mount.here.title')} {...search} />
    {data === undefined ? query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : <SkeletonRows read={query} rows={3} />
      : locations.length === 0 ? <EmptyState compact title={t('resourceDetails.mount.here.none', { machine: name })} />
      : <Table label={t('resourceDetails.mount.here.title')} columns={columns} rows={locations} rowKey={(row) => row.mount.id} virtual key={machine + search.q} rowMenu={{ label: (row) => row.mount.name, entries }} />}
    <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} />
    {data?.available.length === 0 && <EmptyState compact title={t('resourceDetails.mount.here.noChoices')} />}
    {adding && data !== undefined && <AddHere machine={machine} onClose={() => setAdding(false)} />}
    {copying !== null && <CopyMountDataLoader mountId={copying.mount.id} sourceMachine={machine} onClose={() => setCopying(null)} />}
    <Dialog open={removing !== null} onOpenChange={(open) => { if (!open && !remove.isPending) setRemoving(null); }} title={t('resourceDetails.mount.removeLocation')} dismissible={!remove.isPending} footer={<>
      <Button disabled={remove.isPending} onClick={() => setRemoving(null)}>{t('common.cancel')}</Button>
      <Button variant="danger" pending={remove.isPending} disabled={removing?.kind === 'volume' && confirm !== removing.mount.name} onClick={() => remove.mutate()}>{t('resourceDetails.remove')}</Button>
    </>}>
      <p>{t(removing?.kind === 'volume' ? 'resourceDetails.mount.volumeRemoval' : 'resourceDetails.mount.pathRemoval', { name: removing?.mount.name ?? '', machine: name })}</p>
      {removing?.kind === 'volume' && <Input label={t('resourceDetails.mount.confirmName')} value={confirm} onChange={(event) => setConfirm(event.target.value)} />}
      {remove.error !== null && <ErrorAlert error={remove.error} />}
    </Dialog>
  </Section>}</CompoundPickerScope>;
}

export function MountsHere(props: { machine: string; name: string }) {
  const { scope } = useCollectionSearch('mount-locations:' + props.machine);
  return <Here key={scope} {...props} />;
}

/** Each attachment list has its own native continuation; no query is needed for its admitted first page. */
function AttachedEnvironments({ machine, row }: { machine: string; row: MachineMount }) {
  const { t } = useTranslation();
  const search = useCollectionSearch('mount-attachments:' + machine + ':' + row.mount.id);
  const query = useInfiniteQuery({ ...machineMountsPageQuery(machine, 'environments', search.q, row.mount.id),
    ...(search.q === '' ? { initialData: { pages: [{ locations: [row], available: [], grouping: [], next_cursor: row.environments_next_cursor }], pageParams: [null] }, staleTime: Infinity } : {}),
  });
  const rows = search.q === '' && row.environments_next_cursor === null ? row.environments : query.data?.pages.flatMap((page) => page.locations[0]?.environments ?? []) ?? [];
  return <div className="flex min-w-0 flex-col gap-2">
    {(row.environments.length > 10 || row.environments_next_cursor !== null || search.text !== '') && <CollectionSearch name={t('resourceDetails.mount.here.environments')} {...search} />}
    {query.isPending ? <SkeletonRows read={query} rows={2} /> : query.isError && !query.isFetchNextPageError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : rows.length === 0 ? <span className="text-text-muted">{t('resourceDetails.mount.here.noEnvironments')}</span> : <ScrollRegion kind="table" label={t('resourceDetails.mount.here.environments')} className="max-h-40 overflow-y-auto"><ul>{rows.map((environment) => <li key={environment.id} className="flex flex-wrap gap-x-2"><ResourceChip subject={{ kind: 'environment', environment }} asChild hint={false}><Link to="/resources/environments/$id" params={{ id: environment.id }}>{environment.name}</Link></ResourceChip><span className="text-sm text-text-muted">{t(`environments.status.${environment.status}`)}</span></li>)}</ul></ScrollRegion>}
    <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} />
  </div>;
}
