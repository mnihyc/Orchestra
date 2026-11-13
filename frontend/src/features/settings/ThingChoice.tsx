/** The settings whose value names a thing that exists (an environment, an agent profile, mounts, documents) are chosen by name from the product's own lists.
 *
 * The person sees and searches names; the list holds what can be chosen there and greys what cannot with its reason; a value shows as the thing's chip
 * with its name, and as "No longer exists" with a Clear act where the thing is gone. The identifier is never typed: it is written by the choice, in the same
 * shape the setting always had, and stays reachable only in the raw value of a row that has one.
 */
import { useInfiniteQuery, useQueries, useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { FileText, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { ApiError, client } from '@/api/client';
import { documentQuery } from '@/api/documents';
import { environmentQuery, environmentsQuery } from '@/api/environments';
import { everythingQuery } from '@/api/everything';
import { mountsQuery } from '@/api/resource-tools';
import { agentQuery, agentsQuery } from '@/api/resources';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { agentChoice, environmentChoice } from '@/features/resources/choiceAvailability';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { choiceCopy } from '@/i18n/choices';
import { useDebounced } from '@/lib/debounce';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { Select } from '@/ui/Select';

/** The settings that name one thing, and the kind of thing each names. */
export const THING_SETTINGS = { 'env.default_id': 'environment', 'agent.default_agent_id': 'agent' } as const;
export type ThingSetting = keyof typeof THING_SETTINGS;

export function isThingSetting(key: string | undefined): key is ThingSetting {
  return key !== undefined && Object.hasOwn(THING_SETTINGS, key);
}

/** Whether the server answered that the thing is not there, as against a refusal to say whether it is. */
const missing = (error: unknown): boolean => error instanceof ApiError && [404, 410].includes(error.status);
const hidden = (error: unknown): boolean => error instanceof ApiError && [401, 403].includes(error.status);

/** One environment or one agent profile, chosen by name. */
export function ThingSettingChoice({ settingKey, value, label, onChange, onEdited, disabled = false, error }: ValueControlProps & { settingKey: ThingSetting }) {
  const { t } = useTranslation();
  const kind = THING_SETTINGS[settingKey];
  const [open, setOpen] = useState(false);
  const selected = typeof value === 'string' ? value : '';
  const environments = useQuery({ ...environmentsQuery(null), enabled: open && kind === 'environment' });
  const agents = useQuery({ ...agentsQuery(), enabled: open && kind === 'agent' });
  const environment = useQuery({ ...environmentQuery(selected), enabled: selected !== '' && kind === 'environment', retry: false });
  const agent = useQuery({ ...agentQuery(selected), enabled: selected !== '' && kind === 'agent', retry: false });
  const list = kind === 'environment' ? environments : agents;
  const chosen = kind === 'environment' ? environment : agent;
  const options = kind === 'environment' ? (environments.data ?? []).map((row) => environmentChoice(row, t)) : (agents.data ?? []).map((row) => agentChoice(row, t));
  const gone = selected !== '' && missing(chosen.error);
  const name = chosen.data === undefined ? undefined : chosen.data.name;
  return <div className="flex min-w-0 flex-col gap-2" data-thing-choice={kind}>
    <ResourcePicker pickerKey={`settings.thing.${settingKey}`} label={label} hideLabel size="sm" value={selected} open={open && !list.isError} onOpenChange={setOpen}
      valueLabel={selected === '' ? undefined : gone ? t('settingsEditors.thing.gone') : hidden(chosen.error) ? t('settings.compact.managed') : name ?? t(chosen.isError ? 'settings.resourceChoice.unavailable' : 'common.loading')}
      placeholder={open && list.isPending ? t('common.loading') : t('settings.resourceChoice.choose')} disabled={disabled} error={error} loading={list.isFetching}
      options={options} onValueChange={(next) => { onEdited?.(); onChange(next); }} />
    {gone && <div className="flex flex-wrap items-center gap-2 text-sm text-text-muted">
      <span>{t('settingsEditors.thing.goneAbout', { kind: t(`settingsEditors.thing.kinds.${kind}`) })}</span>
      <Button size="sm" variant="ghost" disabled={disabled} onClick={() => { onEdited?.(); onChange(null); }}>{t('settingsEditors.thing.clear')}</Button>
    </div>}
    {list.isError && <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => { setOpen(false); void list.refetch(); }} />}
  </div>;
}

/** What a mount is set to in an environment, as a person says it, and as the setting has always written it. */
const MOUNT_MODES = ['shared', 'read_only', 'private', 'off'] as const;
type MountMode = (typeof MOUNT_MODES)[number];
const modeOf = (value: unknown): MountMode => value === false ? 'off' : value === 'read_only' || value === 'private' ? value : 'shared';
const writtenAs = (mode: MountMode): boolean | string => mode === 'shared' ? true : mode === 'off' ? false : mode;

/** How a mount is attached, in the words of the product. */
function mountModeName(t: TFunction, mode: MountMode): string {
  return mode === 'off' ? t('common.off') : choiceCopy(t, mode === 'shared' ? 'mountMode.shared' : mode === 'read_only' ? 'mountMode.read_only' : 'mountMode.private').name;
}

const mountEntries = (value: unknown): [string, unknown][] => value !== null && typeof value === 'object' && !Array.isArray(value) ? Object.entries(value) : [];

/** The mounts attached beside the workspace, each chosen by name with the way it is attached. */
export function MountSettingChoice({ value, label, onChange, onEdited, disabled = false, error }: ValueControlProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const chosen = mountEntries(value);
  const catalogue = useQuery({ ...mountsQuery, enabled: open || chosen.length > 0 });
  const rows = catalogue.data ?? [];
  const write = (next: [string, unknown][]) => { onEdited?.(); onChange(Object.fromEntries(next)); };
  const modeName = (mode: MountMode) => mountModeName(t, mode);
  return <div className="flex min-w-0 flex-col gap-2" data-thing-choice="mount">
    {chosen.length > 0 && <ul aria-label={label} className="flex flex-col gap-2">{chosen.map(([id, mode]) => {
      const mount = rows.find((row) => row.id === id);
      const gone = mount === undefined && catalogue.isSuccess;
      const name = mount?.name ?? t(gone ? 'settingsEditors.thing.gone' : catalogue.isPending ? 'common.loading' : 'settings.resourceChoice.unavailable');
      return <li key={id} className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="min-w-0 flex-1">{mount === undefined ? name : <ResourceChip subject={{ kind: 'mount', mount }} hint={false} />}</span>
        <Select size="sm" label={t('settingsEditors.thing.attachedAs', { name })} hideLabel value={modeOf(mode)} disabled={disabled}
          options={MOUNT_MODES.map((one) => ({ value: one, label: modeName(one) }))}
          onValueChange={(next) => write(chosen.map(([key, before]) => [key, key === id ? writtenAs(next) : before]))} />
        <IconButton size="sm" label={t('settingsEditors.thing.remove', { name })} icon={<X aria-hidden />} disabled={disabled} onClick={() => write(chosen.filter(([key]) => key !== id))} />
      </li>;
    })}</ul>}
    <ResourcePicker pickerKey="settings.thing.env.mounts" label={t('settingsEditors.thing.addMount')} hideLabel value="" open={open} onOpenChange={setOpen} disabled={disabled} error={error}
      loading={catalogue.isFetching} placeholder={t(open && catalogue.isPending ? 'common.loading' : 'settingsEditors.thing.addMount')}
      options={rows.filter((row) => !chosen.some(([id]) => id === row.id)).map((mount) => ({ value: mount.id, name: mount.name, resource: { kind: 'mount' as const, id: mount.id, data: mount },
        ...resourceChoice({ kind: 'mount', mount }), ...(holds(mount.access, 'mount.use') ? {} : { disabled: true }) }))}
      onValueChange={(id) => write([...chosen, [id, true]])} />
    {catalogue.isError && <ErrorState compact error={catalogue.error} retrying={catalogue.isRefetching} onRetry={() => void catalogue.refetch()} />}
  </div>;
}

/** The documents offered as starting points, each chosen by searching its name among the documents the person can read. */
export function DocumentSetChoice({ value, label, onChange, onEdited, disabled = false, error }: ValueControlProps) {
  const { t } = useTranslation();
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const ids = Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];
  const reads = useQueries({ queries: ids.map((id) => ({ ...documentQuery(id), retry: false })) });
  const [typed, setTyped] = useState('');
  const settled = useDebounced(typed.trim());
  const search = useInfiniteQuery({ ...everythingQuery(reader, null, { scope: 'mine', kinds: ['document'], q: settled, owner: null, createdFrom: '', createdTo: '', sort: 'name', direction: 'asc' }, 'document-set-choices'),
    enabled: settled !== '' && !disabled, retry: false });
  const found = (search.data?.pages.flatMap((page) => page.items) ?? []).filter((row) => row.kind === 'document' && row.name !== null && !ids.includes(row.id));
  const write = (next: string[]) => { onEdited?.(); onChange(next); };
  return <div className="flex min-w-0 flex-col gap-2" data-thing-choice="document">
    {ids.length > 0 && <ul aria-label={label} className="flex flex-col gap-1">{ids.map((id, index) => {
      const read = reads[index];
      const gone = read !== undefined && (missing(read.error) || read.data?.deleted === true);
      const name = read?.data?.title ?? t(gone ? 'settingsEditors.thing.gone' : read !== undefined && hidden(read.error) ? 'settings.compact.managed' : read?.isError === true ? 'settings.resourceChoice.unavailable' : 'common.loading');
      return <li key={id} className="flex min-w-0 items-center gap-2">
        <FileText aria-hidden className="size-4 shrink-0 text-text-muted" />
        <span className="min-w-0 flex-1 wrap-anywhere">{gone ? t('settingsEditors.thing.gone') : name}</span>
        <IconButton size="sm" label={t('settingsEditors.thing.remove', { name })} icon={<X aria-hidden />} disabled={disabled} onClick={() => write(ids.filter((other) => other !== id))} />
      </li>;
    })}</ul>}
    <Combobox label={t('settingsEditors.thing.addDocument')} hideLabel query={typed} onQueryChange={setTyped} minChars={1} disabled={disabled} error={error}
      placeholder={t('settingsEditors.thing.searchDocuments')} loading={search.isFetching} empty={t('ui.combobox.empty')}
      groups={[{ id: 'documents', label: '', options: found.map((row) => ({ value: row.id, label: row.name ?? '', text: row.name ?? '' })) }]}
      onSelect={(option) => { setTyped(''); write([...ids, option.value]); }} />
    {search.isError && <ErrorState compact error={search.error} retrying={search.isRefetching} onRetry={() => void search.refetch()} />}
  </div>;
}

/** The environment or the agent profile a setting names, as the thing's chip with its name. */
export function ThingName({ settingKey, value }: { settingKey: ThingSetting; value: unknown }) {
  const { t } = useTranslation();
  const kind = THING_SETTINGS[settingKey];
  const id = typeof value === 'string' ? value : '';
  const environment = useQuery({ ...environmentQuery(id), enabled: id !== '' && kind === 'environment', retry: false });
  const agent = useQuery({ ...agentQuery(id), enabled: id !== '' && kind === 'agent', retry: false });
  const read = kind === 'environment' ? environment : agent;
  if (id === '') return <span>{t('settings.compact.none')}</span>;
  if (missing(read.error)) return <span>{t('settingsEditors.thing.gone')}</span>;
  if (hidden(read.error)) return <span>{t('settings.compact.managed')}</span>;
  if (environment.data !== undefined && kind === 'environment') return <ResourceChip subject={{ kind: 'environment', environment: environment.data }} hint={false} />;
  if (agent.data !== undefined && kind === 'agent') return <ResourceChip subject={{ kind: 'agent', agent: agent.data }} hint={false} />;
  return <span>{t(read.isError ? 'settings.resourceChoice.unavailable' : 'common.loading')}</span>;
}

/** The mounts a setting names, each with its name and how it is attached. */
export function MountsSummary({ value }: { value: unknown }) {
  const { t } = useTranslation();
  const chosen = mountEntries(value);
  const catalogue = useQuery({ ...mountsQuery, enabled: chosen.length > 0 });
  if (chosen.length === 0) return <span>{t('settings.compact.none')}</span>;
  return <span className="inline-flex min-w-0 flex-col gap-1">{chosen.map(([id, mode]) => {
    const mount = (catalogue.data ?? []).find((row) => row.id === id);
    return <span key={id} className="inline-flex min-w-0 flex-wrap items-center gap-x-1">
      {mount === undefined ? <span>{t(catalogue.isSuccess ? 'settingsEditors.thing.gone' : catalogue.isPending ? 'common.loading' : 'settings.resourceChoice.unavailable')}</span> : <ResourceChip subject={{ kind: 'mount', mount }} hint={false} />}
      <span className="text-text-muted">· {mountModeName(t, modeOf(mode))}</span>
    </span>;
  })}</span>;
}

/** The documents a setting names, by their titles. */
export function DocumentsSummary({ value }: { value: unknown }) {
  const { t } = useTranslation();
  const ids = Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];
  const reads = useQueries({ queries: ids.map((id) => ({ ...documentQuery(id), retry: false })) });
  if (ids.length === 0) return <span>{t('settings.compact.none')}</span>;
  const names = ids.map((id, index) => {
    const read = reads[index];
    if (read === undefined) return t('common.loading');
    if (missing(read.error) || read.data?.deleted === true) return t('settingsEditors.thing.gone');
    return read.data?.title ?? t(hidden(read.error) ? 'settings.compact.managed' : read.isError ? 'settings.resourceChoice.unavailable' : 'common.loading');
  });
  return <span className="min-w-0 wrap-anywhere">{names.join(', ')}</span>;
}
