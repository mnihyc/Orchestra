/** Mount locations on one machine, with reported sizes, readable attachments and the ordinary guarded add/remove acts. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from '@tanstack/react-router';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';
import { machineMountsQuery, removeMountLocation, setMountLocation } from '@/api/resource-tools';
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

function AddHere({ machine, choices, onClose }: { machine: string; choices: readonly MachineMountChoice[]; onClose: () => void }) {
  const { t } = useTranslation();
  const [mount, setMount] = useState('');
  const [kind, setKind] = useState<'path' | 'volume'>('path');
  const [source, setSource] = useState('');
  const [storage, setStorage] = useState('default');
  const choice = choices.find((choice) => choice.mount.id === mount);
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
      <Select label={t('resourceDetails.mount.here.mount')} value={mount} disabled={save.isPending} placeholder={t('resourceDetails.mount.here.choose')}
        options={choices.map((choice) => ({ value: choice.mount.id, ...choiceOption(t, 'dynamic.resource', { name: choice.mount.name }), ...resourceChoice({ kind: 'mount', mount: choice.mount }), description: choice.mount.address === '' ? choiceOption(t, 'dynamic.resource', { name: choice.mount.name }).description : choice.mount.address }))}
        onValueChange={(id) => {
          const next = choices.find((choice) => choice.mount.id === id);
          setMount(id); setKind(next?.path === true ? 'path' : 'volume'); setSource('');
          setStorage(next?.default_storage === true ? 'default' : `named:${next?.storages[0] ?? ''}`);
        }} />
      {choice !== undefined && <Select<'path' | 'volume'> label={t('resourceDetails.mount.locationKind')} value={kind} disabled={save.isPending} onValueChange={setKind}
        options={[...(choice.path ? [{ value: 'path' as const, ...choiceOption(t, 'mountLocation.path') }] : []), ...(canVolume ? [{ value: 'volume' as const, ...choiceOption(t, 'mountLocation.volume') }] : [])]} />}
      {choice !== undefined && (kind === 'path'
        ? <Input label={t('resourceDetails.mount.source')} value={source} disabled={save.isPending} onChange={(event) => setSource(event.target.value)} />
        : <Select label={t('resourceDetails.mount.storage')} value={storage} disabled={save.isPending} onValueChange={setStorage}
          options={[...(choice.default_storage ? [{ value: 'default', ...choiceOption(t, 'mountLocation.default') }] : []), ...choice.storages.map((name) => ({ value: `named:${name}`, ...choiceOption(t, 'dynamic.resource', { name }) }))]} />)}
      {save.error !== null && <ErrorAlert error={save.error} />}
    </div>
  </Dialog>;
}

function Here({ machine, name }: { machine: string; name: string }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const query = useQuery(machineMountsQuery(machine));
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<MachineMount | null>(null);
  const [confirm, setConfirm] = useState('');
  const remove = useMutation({ mutationFn: async () => {
    if (removing === null) return;
    await removeMountLocation(removing.mount.id, machine, removing.kind === 'volume' ? confirm : undefined);
  }, onSuccess: () => setRemoving(null) });
  const data = query.isError ? undefined : query.data;
  const columns: Column<MachineMount>[] = [
    { id: 'mount', header: t('resourceDetails.mount.here.mount'), cell: (row) => <ResourceChip subject={{ kind: 'mount', mount: row.mount }} asChild hint={false}><Link to="/resources/mounts/$id" params={{ id: row.mount.id }}>{row.mount.name}</Link></ResourceChip> },
    { id: 'location', header: t('resourceDetails.mount.locations'), cell: (row) => <div className="flex min-w-0 flex-col gap-1">
      <span>{t(row.kind === 'volume' ? 'resourceDetails.mount.volume' : 'resourceDetails.mount.path')}</span>
      <code className="wrap-anywhere">{row.source ?? t('resourceDetails.mount.sourceHidden')}</code>
      {row.kind === 'volume' && <span className="text-sm text-text-muted">{row.storage ?? t('resourceDetails.mount.defaultStorage')}</span>}
      {row.data_kind !== null && <span className="text-sm text-text-muted">{choiceOption(t, `mountDataKind.${row.data_kind}`).name}</span>}
    </div> },
    { id: 'size', header: t('resourceDetails.mount.here.size'), cell: (row) => row.size_bytes === null ? t('resourceDetails.mount.here.sizeUnknown') : formatBytes(row.size_bytes, locale) },
    { id: 'environments', header: t('resourceDetails.mount.here.environments'), cell: (row) => row.environments.length === 0
      ? <span className="text-text-muted">{t('resourceDetails.mount.here.noEnvironments')}</span>
      : <ul>{row.environments.map((environment) => <li key={environment.id} className="flex flex-wrap gap-x-2"><ResourceChip subject={{ kind: 'environment', environment }} asChild hint={false}><Link to="/resources/environments/$id" params={{ id: environment.id }}>{environment.name}</Link></ResourceChip><span className="text-sm text-text-muted">{t(`environments.status.${environment.status}`)}</span></li>)}</ul> },
    { id: 'remove', header: <span className="sr-only">{t('resourceDetails.remove')}</span>, cell: (row) => row.can_remove && <Button variant="danger-ghost" size="sm" onClick={() => { setRemoving(row); setConfirm(''); remove.reset(); }}>{t('resourceDetails.mount.here.remove', { name: row.mount.name })}</Button> },
  ];
  return <Section id={`mounts-here-${machine}`} title={t('resourceDetails.mount.here.title')} action={<Button variant="primary" icon={<Plus aria-hidden />} disabled={data === undefined || data.available.length === 0} onClick={() => setAdding(true)}>{t('resourceDetails.mount.here.add')}</Button>}>
    {data === undefined ? query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : <SkeletonRows rows={3} />
      : data.locations.length === 0 ? <EmptyState compact title={t('resourceDetails.mount.here.none', { machine: name })} />
      : <Table label={t('resourceDetails.mount.here.title')} columns={columns} rows={data.locations} rowKey={(row) => row.mount.id} />}
    {data?.available.length === 0 && <EmptyState compact title={t('resourceDetails.mount.here.noChoices')} />}
    {adding && data !== undefined && <AddHere machine={machine} choices={data.available} onClose={() => setAdding(false)} />}
    <Dialog open={removing !== null} onOpenChange={(open) => { if (!open && !remove.isPending) setRemoving(null); }} title={t('resourceDetails.mount.removeLocation')} dismissible={!remove.isPending} footer={<>
      <Button disabled={remove.isPending} onClick={() => setRemoving(null)}>{t('common.cancel')}</Button>
      <Button variant="danger" pending={remove.isPending} disabled={removing?.kind === 'volume' && confirm !== removing.mount.name} onClick={() => remove.mutate()}>{t('resourceDetails.remove')}</Button>
    </>}>
      <p>{t(removing?.kind === 'volume' ? 'resourceDetails.mount.volumeRemoval' : 'resourceDetails.mount.pathRemoval', { name: removing?.mount.name ?? '', machine: name })}</p>
      {removing?.kind === 'volume' && <Input label={t('resourceDetails.mount.confirmName')} value={confirm} onChange={(event) => setConfirm(event.target.value)} />}
      {remove.error !== null && <ErrorAlert error={remove.error} />}
    </Dialog>
  </Section>;
}

export function MountsHere(props: { machine: string; name: string }) {
  return <Here key={props.machine} {...props} />;
}
