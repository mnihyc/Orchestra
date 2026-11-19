/** Member consent is edited on its ordinary sharing page, including inherited defaults. */
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useMutation, useQuery, useQueries } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { permissionsQuery, searchPeopleQuery } from '@/api/access';
import { poolLendingQuery, poolConsentNamesQuery, putPoolLending } from '@/api/pools';
import type { PoolLendingScope, PoolLendingView, LendingMatcher, PoolLendingPutBody } from '@/generated/api-types';
import { OptionFields, optionFields, readOptions } from '@/features/access/EntryOptions';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { EditDialog } from '@/features/resources/EditDialog';
import { HomePicker } from '@/features/resources/HomePicker';
import { useDebounced } from '@/lib/debounce';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Combobox } from '@/ui/Combobox';
import { DateField } from '@/ui/DateField';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { PoolResourceChoice } from './ResourceChoice';

const supported = new Set(['provider', 'environment', 'host', 'template', 'pool']);
export const hasPoolLending = (kind: string) => supported.has(kind);
export function PoolLending({ scope }: { scope: PoolLendingScope }) {
  const reader=useStore(client.session,(state)=>`${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const { t } = useTranslation();
  return <ResourceDetails label={t('pools.through')}><LendingEntries key={`${JSON.stringify(scope)}:${reader}`} scope={scope} reader={reader} /></ResourceDetails>;
}
function LendingEntries({ scope,reader }: { scope: PoolLendingScope;reader:string }) {
  const { t } = useTranslation();
  const [after, setAfter] = useState<string | null>(null);
  const query = useQuery(poolLendingQuery(scope, after,reader));
  const next=query.isError ? null : query.data?.next ?? null;
  const rows=query.isError ? [] : query.data?.items ?? [];
  const identity=(matcher:LendingMatcher)=>JSON.stringify(matcher);
  const targets=[...new Map(rows.map((row)=>[identity(row.matcher),row.matcher])).values()];
  const names=useQueries({queries:targets.map((target)=>poolConsentNamesQuery(target,reader))});
  const named=new Map(targets.map((target,index)=>[identity(target),names[index]]));
  const [adding, setAdding] = useState(false);
  const [text, setText] = useState('');
  const revoke = useMutation({ mutationFn: (row: PoolLendingView) => putPoolLending(row.id, { scope, expected_revision: row.revision,
    matcher: row.matcher, options: row.options, show_names: row.show_names, enabled: false, expires_at: row.expires_at }),
    onSuccess: () => { void query.refetch(); } });
  return <div className="flex flex-col gap-3">
    <div className="flex gap-2"><input aria-label={t('pools.searchConsents')} className="min-w-0 flex-1 rounded border border-control-border bg-surface px-2" value={text} onChange={(event) => setText(event.target.value)} />
      <Button size="sm" onClick={() => setAdding(true)}>{t('common.add')}</Button></div>
    {query.isPending ? <SkeletonRows read={query} rows={2} /> : query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> :
      <ul className="max-h-[50vh] overflow-y-auto divide-y divide-border">{query.data.items.filter((row) => { const read=named.get(identity(row.matcher));const labels=read?.isError === false ? read.data ?? [] : [];return [t(`pools.matchers.${row.matcher.kind}`),...labels.map((name)=>name.name)].join(' ').toLocaleLowerCase().includes(text.toLocaleLowerCase());}).map((row) =>
        <li key={row.id} className="flex items-center justify-between gap-2 py-2"><span><span className="text-text-muted">{t(`pools.matchers.${row.matcher.kind}`)} · </span><span>{(() => {const read=named.get(identity(row.matcher));return read?.isError === false && read.data !== undefined && read.data.length>0 ? read.data.map((name)=>name.name).join(t('startFlow.separator')) : t(read?.isPending === true ? 'common.loading' : read?.isError === true ? 'pools.unavailable' : 'pools.hidden');})()}</span></span>
          <Button size="sm" disabled={!row.can_revoke || revoke.isPending} onClick={() => revoke.mutate(row)}>{t('pools.withdraw')}</Button></li>)}</ul>}
    {revoke.isError && <ErrorAlert error={revoke.error} />}
    <div className="flex justify-between"><Button size="sm" disabled={after === null || query.isFetching} onClick={() => setAfter(null)}>{t('documentFolders.beginning')}</Button>
      <Button size="sm" disabled={next === null || query.isFetching} onClick={() => { if (next !== null) setAfter(next); }}>{t('documentFolders.next')}</Button></div>
    {adding && <LendingDialog scope={scope} onSaved={() => void query.refetch()} onClose={() => setAdding(false)} />}
  </div>;
}
function LendingDialog({ scope, onClose, onSaved }: { scope: PoolLendingScope; onClose: () => void; onSaved: () => void }) {
  const { t } = useTranslation();
  const [id] = useState(() => crypto.randomUUID());
  const [matcher, setMatcher] = useState<LendingMatcher['kind']>('pool');
  const [target, setTarget] = useState('');
  const [manager, setManager] = useState<{ id: string; name: string } | null>(null);
  const [text, setText] = useState('');
  const [names, setNames] = useState(false);
  const [until, setUntil] = useState('');
  const [values, setValues] = useState<Record<string, unknown>>({});
  const catalogue = useQuery(permissionsQuery);
  const people = useQuery(searchPeopleQuery(useDebounced(text.trim()), null));
  const kind = scope.kind === 'member' ? scope.object.kind : scope.resource_kind;
  const def = catalogue.data?.permissions.find((permission) => permission.name === `${kind}.use`);
  const fields = optionFields(def?.options_schema);
  const selected: LendingMatcher | null = matcher === 'managers' ? manager === null ? null : { kind: 'managers', manager_ids: [manager.id] }
    : target === '' ? null : matcher === 'pool' ? { kind: 'pool', pool_id: target } : { kind: 'home', home_id: target };
  const save = useMutation({ mutationFn: () => {
    if (selected === null) throw new Error(t('pools.form.invalid'));
    const body: PoolLendingPutBody = { scope, expected_revision: null, matcher: selected, options: readOptions(fields, values),
      show_names: names, enabled: true, expires_at: until === '' ? null : new Date(`${until}T23:59:59`).toISOString() };
    return putPoolLending(id, body);
  }, onSuccess: () => { onSaved(); onClose(); } });
  return <EditDialog title={t('pools.through')} changed valid={selected !== null && catalogue.isSuccess} pending={save.isPending} error={save.error ?? catalogue.error} onSave={() => save.mutate()} onClose={onClose}>
    <Select label={t('pools.destination')} value={matcher} onValueChange={(next) => { setMatcher(next); setTarget(''); }} options={(['pool', 'home', 'managers'] as const).map((value) => ({ value, name: t(`pools.matchers.${value}`) }))} />
    {matcher === 'pool' ? <PoolResourceChoice kind="pools" value={target} onChange={setTarget} label={t('pools.destination')} /> : matcher === 'home'
      ? <HomePicker compact value={target} onChange={setTarget} /> : <Combobox label={t('pools.manager')} query={text} onQueryChange={setText} displayValue={manager?.name ?? ''}
        groups={[{ id: 'people', label: '', options: (people.data ?? []).map((person) => ({ value: person.id, label: person.display_name, text: person.display_name })) }]}
        onSelect={(option) => { const person = people.data?.find((person) => person.id === option.value); if (person !== undefined) { setManager({ id: person.id, name: person.display_name }); setText(''); } }} />}
    <Checkbox label={t('pools.showNames')} checked={names} onCheckedChange={setNames} />
    <ResourceDetails><div className="flex flex-col gap-3"><OptionFields fields={fields} values={values} onChange={setValues} />
      <DateField label={t('pools.expires')} value={until} onValueChange={setUntil} /></div></ResourceDetails>
  </EditDialog>;
}

